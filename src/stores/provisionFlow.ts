import { createStore } from "zustand/vanilla";
import type { ProvisionLink } from "../utils/provisionLink";

/** What a setup link's preview shows (from `provision_link_preview`). Nothing secret. */
export interface ProvisionPreview {
  tenantName: string;
  extension: string;
  displayName: string;
  domain: string;
  expiresAt: string | null;
  serverHost: string;
}

/**
 * A redeemed line (from `provision_link_redeem`). The password is not here:
 * the backend has already saved it under `accountId`.
 */
export interface ProvisionedLine {
  accountId: string;
  /** An existing account with the same username@domain was updated. */
  updated: boolean;
  tenantName: string;
  displayName: string;
  username: string;
  authUsername: string | null;
  domain: string;
  outboundProxy: string | null;
  registrar: string | null;
  port: number;
  transport: string;
  voicemail: string | null;
}

export interface ProvisionApi {
  /** GET: uses nothing up. */
  preview(redeemUrl: string): Promise<ProvisionPreview>;
  /** POST: uses the link up. Only ever called from `confirm`. */
  redeem(redeemUrl: string): Promise<ProvisionedLine>;
  /** Save and register the line. Resolves with the registration error, or null once registered. */
  activate(line: ProvisionedLine): Promise<string | null>;
}

export type ProvisionState =
  | { phase: "idle" }
  | { phase: "loading"; link: ProvisionLink }
  | { phase: "preview"; link: ProvisionLink; preview: ProvisionPreview }
  | { phase: "redeeming"; link: ProvisionLink; preview: ProvisionPreview }
  | { phase: "registering"; link: ProvisionLink; preview: ProvisionPreview; line: ProvisionedLine }
  | { phase: "done"; line: ProvisionedLine; error: string | null }
  | { phase: "failed"; message: string };

export interface ProvisionFlow {
  state: ProvisionState;
  /** Start on a link: fetch its preview. Ignored (returns false) while another link is open. */
  open(link: ProvisionLink): boolean;
  /** The user clicked Add. The only path to `redeem`. */
  confirm(): Promise<void>;
  /** Close the dialog. Ignored mid-redeem, once the link may already be used up. */
  cancel(): void;
}

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

/**
 * The setup-link flow: preview, then confirm, then redeem and register.
 *
 * Any web page can fire an `aria://` link, and the confirm click is the only
 * thing between that and a hostile SIP account being added, so `redeem` is
 * reachable from `confirm` alone, and only from the preview. A second link
 * while one is open is ignored rather than swapped in under the user's cursor.
 */
export function createProvisionFlow(api: ProvisionApi) {
  // Bumped whenever the dialog closes or a new link opens, so a late reply
  // for a cancelled link cannot reopen it.
  let generation = 0;

  return createStore<ProvisionFlow>()((set, get) => ({
    state: { phase: "idle" },

    open(link) {
      if (get().state.phase !== "idle") return false;
      const gen = ++generation;
      set({ state: { phase: "loading", link } });
      api.preview(link.redeemUrl).then(
        (preview) => {
          if (gen !== generation) return;
          set({ state: { phase: "preview", link, preview } });
        },
        (e) => {
          if (gen !== generation) return;
          set({ state: { phase: "failed", message: message(e) } });
        },
      );
      return true;
    },

    async confirm() {
      const current = get().state;
      if (current.phase !== "preview") return;
      const gen = generation;
      const { link, preview } = current;
      set({ state: { phase: "redeeming", link, preview } });

      let line: ProvisionedLine;
      try {
        line = await api.redeem(link.redeemUrl);
      } catch (e) {
        if (gen === generation) set({ state: { phase: "failed", message: message(e) } });
        return;
      }
      if (gen !== generation) return;
      set({ state: { phase: "registering", link, preview, line } });

      let error: string | null;
      try {
        error = await api.activate(line);
      } catch (e) {
        error = message(e);
      }
      if (gen === generation) set({ state: { phase: "done", line, error } });
    },

    cancel() {
      const { phase } = get().state;
      if (phase === "redeeming" || phase === "registering") return;
      generation++;
      set({ state: { phase: "idle" } });
    },
  }));
}

export type ProvisionFlowStore = ReturnType<typeof createProvisionFlow>;
