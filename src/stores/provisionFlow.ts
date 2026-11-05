import { createStore } from "zustand/vanilla";
import type { ProvisionLink } from "../utils/provisionLink";
import type { SipAccount, SrtpMode, TransportType } from "../types/sip";

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
  mode: LineMode;
  /** The setup link's `host[:port]`, recorded on the account. */
  provisionedFrom: string;
  tenantName: string;
  displayName: string;
  username: string;
  authUsername: string | null;
  domain: string;
  outboundProxy: string | null;
  tlsServerName: string | null;
  port: number;
  transport: string;
  voicemail: string | null;
  /**
   * The media encryption the server asks for. `null` when it sent none or a
   * value the backend didn't recognise. See `provisionedSrtpMode`.
   */
  srtp: ProvisionSrtp | null;
}

export type ProvisionSrtp = "sdes" | "disabled";

/**
 * What a setup link does to the app's accounts:
 * - `added`: a new account.
 * - `updated`: the account this same host set up earlier, updated in place.
 * - `separate`: a new account, though one with the same username@domain
 *   exists — set up by hand or by another host, and left alone.
 */
export type LineMode = "added" | "updated" | "separate";

type AccountIdentity = Pick<SipAccount, "username" | "domain" | "provisionedFrom">;

/**
 * What confirming this preview will do, worked out the same way the backend
 * decides it, so the dialog can say so before the click.
 */
export function previewMode(preview: ProvisionPreview, accounts: readonly AccountIdentity[]): LineMode {
  const same = accounts.filter(
    (a) => a.username === preview.extension && a.domain.trim().toLowerCase() === preview.domain.toLowerCase(),
  );
  if (same.some((a) => a.provisionedFrom?.toLowerCase() === preview.serverHost.toLowerCase())) return "updated";
  return same.length > 0 ? "separate" : "added";
}

/**
 * What the dialog says Add will do. Once the line is redeemed it is the
 * backend's answer, not recomputed: by then the new line is itself among the
 * accounts and would read as an update of itself.
 */
export function dialogMode(state: ProvisionState, accounts: readonly AccountIdentity[]): LineMode {
  if ("line" in state) return state.line.mode;
  if ("preview" in state) return previewMode(state.preview, accounts);
  return "added";
}

/**
 * The SRTP mode a redeemed line ends up with:
 * - `sdes` over TLS: `sdes`.
 * - `sdes` over UDP/TCP: not enabled (SDES keys would travel in clear), so
 *   treated as absent. The backend logs a warning.
 * - `disabled`: `disabled`.
 * - absent: the existing line's mode, untouched, or `disabled` for a new line.
 * Whatever the server says, a line that ends up off TLS never keeps `sdes`:
 * an update that moves an SDES line to UDP/TCP turns SRTP off rather than
 * send its keys in clear.
 */
export function provisionedSrtpMode(
  line: ProvisionedLine,
  existing: SipAccount | undefined,
): SrtpMode | undefined {
  const kept = existing ? existing.srtpMode : "disabled";
  const keep = line.transport !== "tls" && kept === "sdes" ? "disabled" : kept;
  switch (line.srtp) {
    case "sdes":
      return line.transport === "tls" ? "sdes" : keep;
    case "disabled":
      return "disabled";
    default:
      return keep;
  }
}

/** How well a line's calls are protected, for the setup dialog. */
export type CallProtection = "srtp" | "tlsOnly" | "none";

export function callProtection(account: Pick<SipAccount, "transport" | "srtpMode">): CallProtection {
  if (account.transport !== "tls") return "none";
  return account.srtpMode === "sdes" ? "srtp" : "tlsOnly";
}

/**
 * The account to save for a redeemed line. An update replaces only the
 * credentials, server settings and (when the server says) SRTP, keeping what
 * the user chose (name, enabled, codecs, recording, dialing rules, realm
 * override).
 */
export function provisionedAccount(line: ProvisionedLine, existing: SipAccount | undefined): SipAccount {
  const server = {
    username: line.username,
    domain: line.domain,
    // Already in the credential store; never kept in app state.
    password: "",
    transport: line.transport as TransportType,
    port: line.port,
    // REGISTER goes to the domain; the proxy carries the traffic and its host
    // is the TLS name.
    registrar: undefined,
    outboundProxy: line.outboundProxy ?? undefined,
    tlsServerName: line.tlsServerName ?? undefined,
    authUsername: line.authUsername ?? undefined,
    provisionedFrom: line.provisionedFrom,
    srtpMode: provisionedSrtpMode(line, existing),
  };
  if (existing) {
    return {
      ...existing,
      ...server,
      voicemailNumber: line.voicemail ?? existing.voicemailNumber,
    };
  }
  return {
    id: line.accountId,
    displayName: line.displayName || line.username,
    enabled: true,
    voicemailNumber: line.voicemail ?? undefined,
    ...server,
  };
}

export interface ProvisionApi {
  /** GET: uses nothing up. */
  preview(redeemUrl: string): Promise<ProvisionPreview>;
  /**
   * POST: uses the link up. Only ever called from `confirm`. The backend
   * refuses a reply for any line but the previewed one.
   */
  redeem(redeemUrl: string, preview: ProvisionPreview): Promise<ProvisionedLine>;
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
  /**
   * Start on a link: fetch its preview. Ignored (returns false) while another
   * link is open, and for a few seconds after one closes.
   */
  open(link: ProvisionLink): boolean;
  /** The user clicked Add. The only path to `redeem`. */
  confirm(): Promise<void>;
  /** Close the dialog. Ignored mid-redeem, once the link may already be used up. */
  cancel(): void;
}

/** How long further setup links are ignored after the dialog closes. */
export const COOLDOWN_MS = 3000;

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
export function createProvisionFlow(
  api: ProvisionApi,
  { now = Date.now, cooldownMs = COOLDOWN_MS }: { now?: () => number; cooldownMs?: number } = {},
) {
  // Bumped whenever the dialog closes or a new link opens, so a late reply
  // for a cancelled link cannot reopen it.
  let generation = 0;
  // Links are ignored until then: a page firing links in a loop cannot keep
  // reopening the dialog (and pulling the window forward) as it is closed.
  let quietUntil = 0;

  return createStore<ProvisionFlow>()((set, get) => ({
    state: { phase: "idle" },

    open(link) {
      if (get().state.phase !== "idle" || now() < quietUntil) return false;
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
        line = await api.redeem(link.redeemUrl, preview);
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
      if (phase !== "idle") quietUntil = now() + cooldownMs;
      set({ state: { phase: "idle" } });
    },
  }));
}

export type ProvisionFlowStore = ReturnType<typeof createProvisionFlow>;
