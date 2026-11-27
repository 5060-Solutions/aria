/**
 * Moving a call between the user's own devices (Aria PBX): the shapes the
 * PBX sends and the pure rules the UI follows. See callMoveStore for the
 * state and hooks/useCallMove for the wiring.
 */

export interface ActiveElsewhereCall {
  id: string;
  state: "active" | "held";
  direction: "inbound" | "outbound";
  remote: { number: string; name: string | null };
  /** RFC 3339. */
  startedAt: string;
  device: { registrationId: string; label: string };
  /** The Call-ID of that device's leg. */
  sipCallId: string;
}

export interface Device {
  registrationId: string;
  label: string;
  online: boolean;
  /** This computer, as the PBX knows it (`current`). */
  current?: boolean;
}

/** A pending move expires after this long without its INVITE. */
export const PENDING_MOVE_MS = 30_000;
/** How often the active calls are polled. */
export const POLL_MS = 15_000;
/** Idle this long, during a call here, is reported as `desktop_idle`. */
export const IDLE_REPORT_SECS = 180;

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const rec = (v: unknown): Record<string, unknown> => (v && typeof v === "object" ? (v as Record<string, unknown>) : {});

/** One active-call object from the PBX; null when it is not one. */
export function parseActiveCall(raw: unknown): ActiveElsewhereCall | null {
  const o = rec(raw);
  const device = rec(o.device);
  const remote = rec(o.remote);
  const id = str(o.id);
  // A call whose device the PBX could not match to a registration is still a
  // call on another device: the banner shows it, only moving it away needs the id.
  if (!id || !(str(device.registration_id) || str(o.sip_call_id))) return null;
  return {
    id,
    state: o.state === "held" ? "held" : "active",
    direction: o.direction === "outbound" ? "outbound" : "inbound",
    remote: { number: str(remote.number), name: typeof remote.name === "string" && remote.name ? remote.name : null },
    startedAt: str(o.started_at),
    device: { registrationId: str(device.registration_id), label: str(device.label) || str(device.user_agent) },
    sipCallId: str(o.sip_call_id),
  };
}

export function parseActiveCalls(raw: unknown): ActiveElsewhereCall[] {
  return Array.isArray(raw) ? raw.map(parseActiveCall).filter((c): c is ActiveElsewhereCall => c !== null) : [];
}

export function parseDevices(raw: unknown): Device[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map((d) => {
      const o = rec(d);
      return { registrationId: str(o.registration_id), label: str(o.label) || str(o.user_agent), online: o.online !== false, current: o.current === true };
    })
    .filter((d) => d.registrationId);
}

/** The digits of a number that identify it across spellings (`+1 303…`, `303…`). */
function tail(n: string): string {
  const d = n.replace(/\D/g, "");
  return d.length > 10 ? d.slice(-10) : d;
}

/**
 * The calls to offer "Move here" for: not this app's own dialog. A call the
 * PBX could not tie to a registration (empty `registrationId`) cannot be told
 * from this computer's by Call-ID alone while a dialog is ringing or changing,
 * so it is dropped when this app has any live call to the same remote number.
 */
export function callsElsewhere(
  calls: ActiveElsewhereCall[],
  ownSipCallIds: string[],
  ownRegistrationId: string | null,
  ownRemoteNumbers: string[] = [],
): ActiveElsewhereCall[] {
  const own = new Set(ownSipCallIds.filter(Boolean));
  const remotes = new Set(ownRemoteNumbers.map(tail).filter(Boolean));
  return calls.filter((c) => {
    if (c.sipCallId && own.has(c.sipCallId)) return false;
    if (c.device.registrationId === ownRegistrationId) return false;
    if (!c.device.registrationId && remotes.has(tail(c.remote.number))) return false;
    return true;
  });
}

/** The name to show for the far party. */
export function remoteLabel(call: ActiveElsewhereCall): string {
  return call.remote.name || call.remote.number;
}

/** When the call started, in the app's clock (ms), or null when unusable. */
export function startedAtMs(call: Pick<ActiveElsewhereCall, "startedAt">): number | null {
  const t = Date.parse(call.startedAt);
  return Number.isFinite(t) ? t : null;
}

export type CallEvent =
  | { type: "call_active_elsewhere"; call: ActiveElsewhereCall }
  | { type: "call_moved"; callId: string; fromRegistrationId: string; toRegistrationId: string }
  | { type: "call_ended"; callId: string }
  | { type: "call_move_failed"; callId: string; moveId: string; detail: string }
  | { type: "handoff_suggested"; raw: Record<string, unknown> };

/** A WebSocket event as the PBX sent it; null for anything else. */
export function parseCallEvent(raw: unknown): CallEvent | null {
  const o = rec(raw);
  switch (o.type) {
    case "call_active_elsewhere": {
      const call = parseActiveCall(o.call);
      return call ? { type: "call_active_elsewhere", call } : null;
    }
    case "call_moved":
      return { type: "call_moved", callId: str(o.call_id), fromRegistrationId: str(o.from_registration_id), toRegistrationId: str(o.to_registration_id) };
    case "call_ended":
      return { type: "call_ended", callId: str(o.call_id) };
    case "call_move_failed":
      return { type: "call_move_failed", callId: str(o.call_id), moveId: str(o.move_id), detail: str(o.detail) };
    case "handoff_suggested":
      return { type: "handoff_suggested", raw: o };
    default:
      return null;
  }
}

/** The list after an event: a call appears, moves, or goes. */
export function applyToCalls(calls: ActiveElsewhereCall[], event: CallEvent): ActiveElsewhereCall[] {
  switch (event.type) {
    case "call_active_elsewhere":
      return [...calls.filter((c) => c.id !== event.call.id), event.call];
    case "call_ended":
      return calls.filter((c) => c.id !== event.callId);
    case "call_moved":
      // Where it is now arrives as a call_active_elsewhere or on the next
      // poll; until then it is not shown as being on the old device.
      return calls.filter((c) => c.id !== event.callId);
    default:
      return calls;
  }
}

/** A move this app asked for and is waiting on. */
export interface PendingMove {
  moveId: string;
  callId: string;
  /** The original call's start, so the timer carries over. */
  startedAt: number | null;
  expiresAt: number;
  /** True when the move is to this computer (its INVITE is auto-answered). */
  toThisDevice: boolean;
}

/** How often a pending move's status is read from the PBX. */
export const MOVE_STATUS_POLL_MS = 2_000;

/** What `GET /me/moves/{id}` and the WebSocket tell about a pending move. */
export type MoveSignal =
  | { kind: "poll"; status: string; detail: string }
  | { kind: "failed_event"; moveId: string; callId: string; detail: string }
  | { kind: "moved_event"; callId: string }
  | { kind: "call_ended"; callId: string }
  /** This app answered the INVITE of move `moveId`. */
  | { kind: "answered"; moveId: string }
  /** The hard limit fired. */
  | { kind: "timeout" }
  /** Any moment: the limit is checked against `now`. */
  | { kind: "tick" };

/** How a pending move ended; `detail` is the PBX's reason for a failure. */
export type MoveOutcome = { kind: "moved" | "ended" } | { kind: "failed" | "timeout"; detail: string };

/** `GET /me/moves/{id}` body: `status` is pending|ringing|moved|failed. */
export function parseMoveStatus(raw: unknown): { status: string; detail: string } {
  const o = rec(raw);
  return { status: str(o.status), detail: str(o.detail) };
}

/**
 * The pure rule for a pending move: given a signal, how (if at all) it ended.
 * Null means keep waiting. A network error on the poll is no signal at all,
 * so it never extends the 30 s limit (`expiresAt`, set at the 202).
 */
export function settleMove(p: PendingMove | null, signal: MoveSignal, now: number): MoveOutcome | null {
  if (!p) return null;
  switch (signal.kind) {
    case "poll":
      if (signal.status === "moved") return { kind: "moved" };
      if (signal.status === "failed") return { kind: "failed", detail: signal.detail };
      break;
    case "failed_event":
      if (signal.moveId.toLowerCase() === p.moveId) {
        return { kind: "failed", detail: signal.detail };
      }
      break;
    case "moved_event":
      if (signal.callId === p.callId) return { kind: "moved" };
      break;
    case "call_ended":
      if (signal.callId === p.callId) return { kind: "ended" };
      break;
    case "answered":
      if (signal.moveId.trim().toLowerCase() === p.moveId) return { kind: "moved" };
      break;
    case "timeout":
      return { kind: "timeout", detail: "" };
    case "tick":
      break;
  }
  return now >= p.expiresAt ? { kind: "timeout", detail: "" } : null;
}

export function pendingActive(p: PendingMove | null, now: number): PendingMove | null {
  return p && now < p.expiresAt ? p : null;
}

/** What the person is told when a move request is refused. */
export function moveFailureKey(status: number | undefined): string {
  switch (status) {
    case 404:
      return "callMove.errNotFound";
    case 409:
      return "callMove.errBusy";
    case 422:
      return "callMove.errDevice";
    default:
      return "callMove.errGeneric";
  }
}

/** Devices to offer in "Move to...": online, and not this one. */
export function otherOnlineDevices(devices: Device[], ownRegistrationId: string | null): Device[] {
  return devices.filter((d) => d.online && !d.current && d.registrationId !== ownRegistrationId);
}

/** What to report to the PBX about this computer during a call here. */
export type ContextKind = "desktop_locked" | "desktop_idle";

/**
 * The context event to send now, or null. Each kind is sent once per call:
 * `sent` holds what already went. Locked wins over idle.
 */
export function contextToReport(
  ctx: { locked: boolean; idleSecs: number },
  sent: ReadonlySet<ContextKind>,
  idleThresholdSecs = IDLE_REPORT_SECS,
): ContextKind | null {
  if (ctx.locked && !sent.has("desktop_locked")) return "desktop_locked";
  if (!ctx.locked && ctx.idleSecs >= idleThresholdSecs && !sent.has("desktop_idle")) return "desktop_idle";
  return null;
}

/** What `/me/call-handoff` holds (`auto_move_on_leave` is the phones' and is left as the server has it). */
export interface HandoffSettings {
  suggestNearby: boolean;
  /** `auto_move_on_desktop_lock`. */
  autoMoveOnLock: boolean;
  /** `ble_opt_in`. */
  bleOptIn: boolean;
}

export const DEFAULT_HANDOFF: HandoffSettings = { suggestNearby: true, autoMoveOnLock: false, bleOptIn: false };

export function parseHandoff(raw: unknown): HandoffSettings {
  const o = rec(raw);
  return {
    // suggest_nearby defaults to true on the server; the rest to false.
    suggestNearby: o.suggest_nearby !== false,
    autoMoveOnLock: o.auto_move_on_desktop_lock === true,
    bleOptIn: o.ble_opt_in === true,
  };
}

export function handoffBody(s: HandoffSettings) {
  return { suggest_nearby: s.suggestNearby, auto_move_on_desktop_lock: s.autoMoveOnLock, ble_opt_in: s.bleOptIn };
}

/** "Continue on <device>" from a `handoff_suggested` event. */
export interface HandoffSuggestion {
  callId: string;
  device: Device;
  reason: "nearby" | "left_network" | "desktop_locked" | "";
}

/** `{"type":"handoff_suggested","call_id","to_registration_id","reason"}`. */
export function parseSuggestion(raw: Record<string, unknown>, devices: Device[]): HandoffSuggestion | null {
  const registrationId = str(raw.to_registration_id);
  const callId = str(raw.call_id);
  if (!registrationId || !callId) return null;
  const label = devices.find((d) => d.registrationId === registrationId)?.label ?? "";
  const r = str(raw.reason);
  const reason = r === "nearby" || r === "left_network" || r === "desktop_locked" ? r : "";
  return { callId, device: { registrationId, label, online: true }, reason };
}

/** `?sip_instance=<urn>`: every `devices/current/*` route names the device this way. */
export function withSipInstance(path: string, sipInstance: string): string {
  return `${path}?sip_instance=${encodeURIComponent(sipInstance)}`;
}

/** Body of `POST /me/devices/current/proximity`: a hash, never raw network details. */
export function proximityBody(networkHash: string) {
  return { network_hash: networkHash };
}

/** Body of `POST /me/devices/current/context`. */
export function contextBody(kind: ContextKind) {
  return { event: kind };
}

/** The answer to a context report. */
export interface ContextResult {
  action: "moved" | "suggested" | "none";
  callId: string | null;
  moveId: string | null;
}

export function parseContextResult(raw: unknown): ContextResult {
  const o = rec(raw);
  const action = o.action === "moved" || o.action === "suggested" ? o.action : "none";
  return { action, callId: str(o.call_id) || null, moveId: str(o.move_id) || null };
}

/** A call_moved from this device counts for a BYE this soon after it, or before it. */
export const MOVED_WINDOW_MS = 15_000;

/** What a `call_moved` for this device's own leg remembered. */
export interface MovedTo {
  label: string;
  at: number;
  /** The Call-ID of the leg that moved; null when the list did not know it. */
  sipCallId: string | null;
}

/**
 * Whether a call that just ended was moved away, from the `call_moved` seen
 * for this device: the device's label ("" when unknown), or null for an
 * ordinary end. The PBX's BYE and its event can arrive in either order, and
 * only the leg the event named counts (any leg when it is unknown).
 */
export function movedAway(moved: MovedTo | null, now: number, sipCallId: string | null | undefined): string | null {
  if (!moved || Math.abs(now - moved.at) >= MOVED_WINDOW_MS) return null;
  if (moved.sipCallId && sipCallId && moved.sipCallId !== sipCallId) return null;
  return moved.label;
}
