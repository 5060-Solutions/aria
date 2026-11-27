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
  MOVE_STATUS_POLL_MS,
  PENDING_MOVE_MS,
  parseMoveStatus,
  settleMove,
  type MoveSignal,
  applyToCalls,
  contextBody,
  parseContextResult,
  handoffBody,
  type ContextKind,
  type ContextResult,
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
  type MovedTo,
  type PendingMove,
  withSipInstance,
} from "../utils/callMove";

/** This device's registration id is re-read after this long, and before every move. */
export const CURRENT_TTL_MS = 60_000;

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

/** The pending move's timers: the hard limit, and the next status read (chained, never stacked). */
let moveTimers: { limit: ReturnType<typeof setTimeout>; next: ReturnType<typeof setTimeout> | null; stopped: boolean } | null = null;

function stopMoveTimers() {
  if (!moveTimers) return;
  moveTimers.stopped = true;
  clearTimeout(moveTimers.limit);
  if (moveTimers.next) clearTimeout(moveTimers.next);
  moveTimers = null;
}

interface CallMoveState {
  /** The account everything below belongs to; another account starts empty. */
  accountId: string | null;
  /** Every call the PBX says is active for the user, this device's included. */
  calls: ActiveElsewhereCall[];
  devices: Device[];
  /** This device's registration id at the PBX, and when it was read. */
  current: string | null;
  currentAt: number;
  /** A move request is being posted: no second one until it is answered. */
  requesting: boolean;
  pending: PendingMove | null;
  /** Set when the PBX moved this device's call away: the device it went to. */
  movedTo: MovedTo | null;
  suggestion: HandoffSuggestion | null;
  handoff: HandoffSettings;
  handoffLoaded: boolean;

  /** Make the store the given account's: anything from another account is dropped. */
  forAccount: (accountId: string | null) => void;
  refreshCalls: (mailbox: Mailbox) => Promise<void>;
  /** Resolves to whether the list was read. */
  loadDevices: (mailbox: Mailbox) => Promise<boolean>;
  /** This device's registration id, read again when stale or `fresh`. */
  ensureCurrent: (mailbox: Mailbox, fresh?: boolean) => Promise<string | null>;
  /** Ask the PBX to move `callId` to the device `toRegistrationId`. */
  move: (mailbox: Mailbox, call: ActiveElsewhereCall, toRegistrationId: string) => Promise<boolean>;
  moveHere: (mailbox: Mailbox, call: ActiveElsewhereCall) => Promise<boolean>;
  /** Watch a move to another device: polled and timed out like any pending move. */
  trackMove: (mailbox: Mailbox, callId: string, moveId: string, toThisDevice?: boolean, startedAt?: number | null) => void;
  beginPending: (callId: string, moveId: string, toThisDevice: boolean, startedAt: number | null) => void;
  watchPending: (mailbox: Mailbox) => void;
  /** Feed a signal to the pending move; it ends (with a message when it failed) as the rule says. */
  settle: (signal: MoveSignal) => void;
  /** Read the pending move's status from the PBX once. */
  pollPending: (mailbox: Mailbox) => Promise<void>;
  /** Register a move this app started, so its INVITE is answered without ringing. */
  expectMove: (mailbox: Mailbox, callId: string, moveId: string, startedAt?: number | null) => Promise<void>;
  /** Report a context event (`devices/current/context`); a `moved` answer is registered as started here. */
  reportContext: (mailbox: Mailbox, kind: ContextKind) => Promise<ContextResult>;
  /** The pending move an incoming `X-Aria-Move` belongs to, if still live. */
  pendingFor: (moveId: string, now?: number) => PendingMove | null;
  clearPending: () => void;
  applyEvent: (event: CallEvent, mailbox: Mailbox | null) => void;
  dismissSuggestion: () => void;
  clearMovedTo: () => void;
  loadHandoff: (mailbox: Mailbox) => Promise<void>;
  saveHandoff: (mailbox: Mailbox, next: HandoffSettings) => Promise<void>;
}

const EMPTY = {
  calls: [] as ActiveElsewhereCall[],
  devices: [] as Device[],
  current: null,
  currentAt: 0,
  requesting: false,
  pending: null,
  movedTo: null,
  suggestion: null,
  handoff: DEFAULT_HANDOFF,
  handoffLoaded: false,
};

export const useCallMoveStore = create<CallMoveState>((set, get) => ({
  accountId: null,
  ...EMPTY,

  forAccount: (accountId) => {
    if (get().accountId === accountId) return;
    stopMoveTimers();
    set({ ...EMPTY, accountId });
  },

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
      const urn = await invoke<string>("sip_instance_urn");
      set({ devices: parseDevices(await meRequest<unknown>(mailbox, "GET", withSipInstance("/me/devices", urn))) });
      return true;
    } catch (e) {
      log.warn("[callMove] devices not loaded:", e);
      return false;
    }
  },

  ensureCurrent: async (mailbox, fresh = false) => {
    const { current, currentAt } = get();
    if (current && !fresh && Date.now() - currentAt < CURRENT_TTL_MS) return current;
    try {
      const urn = await invoke<string>("sip_instance_urn");
      const raw = await meRequest<{ registration_id?: string }>(
        mailbox,
        "GET",
        withSipInstance("/me/devices/current", urn),
      );
      const id = typeof raw?.registration_id === "string" && raw.registration_id ? raw.registration_id : null;
      set({ current: id, currentAt: Date.now() });
      return id;
    } catch (e) {
      log.warn("[callMove] this device is not known to the PBX:", e);
      return null;
    }
  },

  move: async (mailbox, call, toRegistrationId) => {
    // One request at a time: a second click while the first is being posted
    // would start two moves.
    if (get().requesting) return false;
    set({ requesting: true });
    // Our own id is re-read first: it changes when this device re-registers,
    // and a cached one could even be another account's.
    const own = await get().ensureCurrent(mailbox, true);
    // The PBX may INVITE before the 202 arrives: tell the backend a request
    // is in flight so that INVITE is held for the move id instead of ringing.
    const toThisDevice = toRegistrationId === own;
    if (toThisDevice) await invoke("callmove_request_started", { accountId: mailbox.accountId });
    try {
      const res = await meRequest<{ move_id?: string }>(mailbox, "POST", `/me/calls/${encodeURIComponent(call.id)}/move`, {
        to_registration_id: toRegistrationId,
      });
      const moveId = res?.move_id;
      if (!moveId) throw new Error("no move_id");
      // Only a move to this device is answered here without ringing.
      if (toThisDevice) await get().expectMove(mailbox, call.id, moveId, startedAtMs(call));
      else get().trackMove(mailbox, call.id, moveId);
      return true;
    } catch (e) {
      log.warn("[callMove] move refused:", e);
      notifyError(i18n.t(moveFailureKey(statusOf(e))));
      return false;
    } finally {
      set({ requesting: false });
      if (toThisDevice) await invoke("callmove_request_finished", { accountId: mailbox.accountId }).catch(() => {});
    }
  },

  moveHere: async (mailbox, call) => {
    if (get().requesting) return false;
    const own = await get().ensureCurrent(mailbox, true);
    if (!own) {
      notifyError(i18n.t("callMove.errDevice"));
      return false;
    }
    return get().move(mailbox, call, own);
  },

  expectMove: async (mailbox, callId, moveId, startedAt) => {
    // The store knows the move before the backend does: registering the id
    // releases the held INVITE at once, and its "incoming" event reads the
    // pending move for the call's original start.
    const call = get().calls.find((c) => c.id === callId);
    get().beginPending(callId, moveId, true, startedAt ?? (call ? startedAtMs(call) : null));
    const id = moveId.toLowerCase();
    try {
      await invoke("callmove_expect", { accountId: mailbox.accountId, moveId });
    } catch (e) {
      if (get().pending?.moveId === id) get().clearPending();
      throw e;
    }
    // The limit and the polling start only now, once the backend holds the id.
    if (get().pending?.moveId === id) get().watchPending(mailbox);
  },

  trackMove: (mailbox, callId, moveId, toThisDevice = false, startedAt = null) => {
    get().beginPending(callId, moveId, toThisDevice, startedAt);
    get().watchPending(mailbox);
  },

  beginPending: (callId, moveId, toThisDevice, startedAt) => {
    // A second move replaces the first: the first is revoked, never answered.
    get().clearPending();
    set({ pending: { moveId: moveId.toLowerCase(), callId, startedAt, expiresAt: Date.now() + PENDING_MOVE_MS, toThisDevice } });
  },

  watchPending: (mailbox) => {
    const p = get().pending;
    if (!p) return;
    stopMoveTimers();
    // The limit counts from here: for a move to this device, from when the
    // backend took the id, so it always ends before the backend's own expiry.
    set({ pending: { ...p, expiresAt: Date.now() + PENDING_MOVE_MS } });
    const timers = {
      stopped: false,
      next: null as ReturnType<typeof setTimeout> | null,
      limit: setTimeout(() => {
        if (!timers.stopped) get().settle({ kind: "timeout" });
      }, PENDING_MOVE_MS),
    };
    moveTimers = timers;
    const schedule = () => {
      timers.next = setTimeout(async () => {
        await get().pollPending(mailbox);
        if (!timers.stopped) schedule();
      }, MOVE_STATUS_POLL_MS);
    };
    schedule();
  },

  settle: (signal) => {
    const outcome = settleMove(get().pending, signal, Date.now());
    if (!outcome) return;
    get().clearPending();
    if (outcome.kind === "failed" || outcome.kind === "timeout") {
      const key = outcome.detail ? "callMove.failed" : "callMove.failedNoDetail";
      notifyError(i18n.t(key, { detail: outcome.detail }));
    }
  },

  pollPending: async (mailbox) => {
    const p = get().pending;
    if (!p) return;
    try {
      const raw = await meRequest<unknown>(mailbox, "GET", `/me/moves/${encodeURIComponent(p.moveId)}`);
      // A reply for a move that is no longer the pending one says nothing.
      if (get().pending?.moveId !== p.moveId) return;
      get().settle({ kind: "poll", ...parseMoveStatus(raw) });
    } catch (e) {
      // Not a signal: the 30 s limit runs on regardless.
      log.warn("[callMove] move status not read:", e);
    }
  },

  reportContext: async (mailbox, kind) => {
    const urn = await invoke<string>("sip_instance_urn");
    // A `moved` answer means an INVITE may be on its way ahead of this reply.
    await invoke("callmove_request_started", { accountId: mailbox.accountId });
    try {
      const res = parseContextResult(
        await meRequest<unknown>(mailbox, "POST", withSipInstance("/me/devices/current/context", urn), contextBody(kind)),
      );
      if (res.action === "moved" && res.callId && res.moveId) await get().expectMove(mailbox, res.callId, res.moveId);
      return res;
    } finally {
      await invoke("callmove_request_finished", { accountId: mailbox.accountId }).catch(() => {});
    }
  },

  pendingFor: (moveId, now = Date.now()) => {
    const p = pendingActive(get().pending, now);
    return p && p.moveId === moveId.trim().toLowerCase() ? p : null;
  },

  clearPending: () => {
    stopMoveTimers();
    const p = get().pending;
    if (!p) return;
    set({ pending: null });
    // Revoked in the backend too: a late INVITE for it rings, never auto-answers.
    // A move that was answered has used its id up already.
    const accountId = get().accountId;
    if (accountId) void invoke("callmove_forget", { accountId, moveId: p.moveId }).catch(() => {});
  },

  applyEvent: (event, mailbox) => {
    if (event.type === "handoff_suggested") {
      const s = parseSuggestion(event.raw, get().devices);
      if (s) set({ suggestion: s });
      return;
    }
    if (event.type === "call_move_failed") {
      get().settle({ kind: "failed_event", moveId: event.moveId, callId: event.callId, detail: event.detail });
      return;
    }
    if (event.type === "call_ended") get().settle({ kind: "call_ended", callId: event.callId });
    if (event.type === "call_moved") get().settle({ kind: "moved_event", callId: event.callId });
    // A suggestion is for one call: gone with it.
    const suggestion = get().suggestion;
    if ((event.type === "call_ended" || event.type === "call_moved") && suggestion?.callId === event.callId) {
      set({ suggestion: null });
    }
    if (event.type === "call_moved" && get().current && event.fromRegistrationId === get().current) {
      const label = get().devices.find((d) => d.registrationId === event.toRegistrationId)?.label ?? "";
      const sipCallId = get().calls.find((c) => c.id === event.callId)?.sipCallId || null;
      // The BYE may have come first: that leg is already gone, so say it now.
      const live = useAppStore
        .getState()
        .activeCalls.some((c) => c.state !== "ended" && (!sipCallId || !c.sipCallId || c.sipCallId === sipCallId));
      if (live) {
        set({ movedTo: { label, at: Date.now(), sipCallId } });
      } else {
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
