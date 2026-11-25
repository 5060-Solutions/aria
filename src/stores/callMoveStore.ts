import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import i18n from "../i18n";
import { log } from "../utils/log";
import { ensureBound } from "../utils/pbxBinding";
import { notifyError, useToastStore } from "./toastStore";
import { useAppStore } from "./appStore";
import type { Mailbox } from "../utils/voicemail";
import {
  DEFAULT_HANDOFF,
  PENDING_MOVE_MS,
  applyToCalls,
  handoffBody,
  moveFailureKey,
  parseActiveCalls,
  parseDevices,
  parseHandoff,
  parseSuggestion,
  pendingActive,
  startedAtMs,
  type ActiveElsewhereCall,
  type CallEvent,
  type Device,
  type HandoffSettings,
  type HandoffSuggestion,
  type PendingMove,
} from "../utils/callMove";

/** One `/api/v1/me/...` route through the backend, which holds the session. */
export async function meRequest<T>(mailbox: Mailbox, method: string, path: string, body?: unknown): Promise<T> {
  await ensureBound(mailbox);
  return invoke<T>("pbx_me_request", { mailbox, method, path, body: body ?? null });
}

function statusOf(e: unknown): number | undefined {
  return e && typeof e === "object" && "status" in e && typeof (e as { status: unknown }).status === "number"
    ? (e as { status: number }).status
    : undefined;
}

interface CallMoveState {
  /** Every call the PBX says is active for the user, this device's included. */
  calls: ActiveElsewhereCall[];
  devices: Device[];
  /** This device's registration id at the PBX. */
  current: string | null;
  pending: PendingMove | null;
  /** Set when the PBX moved this device's call away: the device it went to. */
  movedTo: { label: string; at: number } | null;
  suggestion: HandoffSuggestion | null;
  handoff: HandoffSettings;
  handoffLoaded: boolean;

  refreshCalls: (mailbox: Mailbox) => Promise<void>;
  loadDevices: (mailbox: Mailbox) => Promise<void>;
  ensureCurrent: (mailbox: Mailbox) => Promise<string | null>;
  /** Ask the PBX to move `callId` to the device `toRegistrationId`. */
  move: (mailbox: Mailbox, call: ActiveElsewhereCall, toRegistrationId: string) => Promise<boolean>;
  moveHere: (mailbox: Mailbox, call: ActiveElsewhereCall) => Promise<boolean>;
  /** The pending move an incoming `X-Aria-Move` belongs to, if still live. */
  pendingFor: (moveId: string, now?: number) => PendingMove | null;
  clearPending: () => void;
  applyEvent: (event: CallEvent, mailbox: Mailbox | null) => void;
  dismissSuggestion: () => void;
  clearMovedTo: () => void;
  loadHandoff: (mailbox: Mailbox) => Promise<void>;
  saveHandoff: (mailbox: Mailbox, next: HandoffSettings) => Promise<void>;
}

export const useCallMoveStore = create<CallMoveState>((set, get) => ({
  calls: [],
  devices: [],
  current: null,
  pending: null,
  movedTo: null,
  suggestion: null,
  handoff: DEFAULT_HANDOFF,
  handoffLoaded: false,

  refreshCalls: async (mailbox) => {
    try {
      const raw = await meRequest<unknown>(mailbox, "GET", "/me/calls/active");
      set({ calls: parseActiveCalls(raw) });
    } catch (e) {
      log.warn("[callMove] active calls not loaded:", e);
    }
  },

  loadDevices: async (mailbox) => {
    try {
      set({ devices: parseDevices(await meRequest<unknown>(mailbox, "GET", "/me/devices")) });
    } catch (e) {
      log.warn("[callMove] devices not loaded:", e);
    }
  },

  ensureCurrent: async (mailbox) => {
    if (get().current) return get().current;
    try {
      const urn = await invoke<string>("sip_instance_urn");
      const raw = await meRequest<{ registration_id?: string }>(
        mailbox,
        "GET",
        `/me/devices/current?sip_instance=${encodeURIComponent(urn)}`,
      );
      const id = typeof raw?.registration_id === "string" && raw.registration_id ? raw.registration_id : null;
      set({ current: id });
      return id;
    } catch (e) {
      log.warn("[callMove] this device is not known to the PBX:", e);
      return null;
    }
  },

  move: async (mailbox, call, toRegistrationId) => {
    try {
      const res = await meRequest<{ move_id?: string }>(mailbox, "POST", `/me/calls/${encodeURIComponent(call.id)}/move`, {
        to_registration_id: toRegistrationId,
      });
      const moveId = res?.move_id;
      if (!moveId) throw new Error("no move_id");
      // Only a move to this device is answered here without ringing.
      if (toRegistrationId === get().current) {
        await invoke("callmove_expect", { moveId });
        set({
          pending: { moveId: moveId.toLowerCase(), callId: call.id, startedAt: startedAtMs(call), expiresAt: Date.now() + PENDING_MOVE_MS },
        });
      }
      return true;
    } catch (e) {
      log.warn("[callMove] move refused:", e);
      notifyError(i18n.t(moveFailureKey(statusOf(e))));
      return false;
    }
  },

  moveHere: async (mailbox, call) => {
    const own = await get().ensureCurrent(mailbox);
    if (!own) {
      notifyError(i18n.t("callMove.errDevice"));
      return false;
    }
    return get().move(mailbox, call, own);
  },

  pendingFor: (moveId, now = Date.now()) => {
    const p = pendingActive(get().pending, now);
    return p && p.moveId === moveId.trim().toLowerCase() ? p : null;
  },

  clearPending: () => set({ pending: null }),

  applyEvent: (event, mailbox) => {
    if (event.type === "handoff_suggested") {
      const s = parseSuggestion(event.raw, get().devices);
      if (s) set({ suggestion: s });
      return;
    }
    if (event.type === "call_moved" && get().current && event.fromRegistrationId === get().current) {
      const label = get().devices.find((d) => d.registrationId === event.toRegistrationId)?.label ?? "";
      set({ movedTo: { label, at: Date.now() } });
      // The BYE may have come first: the call is already ended, so say it now.
      const calls = useAppStore.getState().activeCalls;
      if (calls.length > 0 && calls.every((c) => c.state === "ended")) {
        set({ movedTo: null });
        useToastStore.getState().show(i18n.t("callMove.movedTo", { device: label || i18n.t("callMove.anotherDevice") }), "info");
      }
      // The label may not be known yet: look the device list up.
      if (!label && mailbox) void get().loadDevices(mailbox);
    }
    set((s) => ({ calls: applyToCalls(s.calls, event) }));
    // After a move the call's new home arrives with the next list.
    if (event.type === "call_moved" && mailbox) void get().refreshCalls(mailbox);
  },

  dismissSuggestion: () => set({ suggestion: null }),
  clearMovedTo: () => set({ movedTo: null }),

  loadHandoff: async (mailbox) => {
    try {
      set({ handoff: parseHandoff(await meRequest<unknown>(mailbox, "GET", "/me/call-handoff")), handoffLoaded: true });
    } catch (e) {
      log.warn("[callMove] handoff settings not loaded:", e);
    }
  },

  saveHandoff: async (mailbox, next) => {
    const before = get().handoff;
    set({ handoff: next });
    try {
      const saved = await meRequest<unknown>(mailbox, "PUT", "/me/call-handoff", handoffBody(next));
      set({ handoff: saved ? parseHandoff(saved) : next });
    } catch (e) {
      log.warn("[callMove] handoff settings not saved:", e);
      set({ handoff: before });
      notifyError(i18n.t("callMove.errGeneric"));
    }
  },
}));
