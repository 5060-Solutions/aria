import { describe, expect, it } from "vitest";
import {
  PENDING_MOVE_MS,
  applyToCalls,
  callsElsewhere,
  contextToReport,
  movedAway,
  moveFailureKey,
  otherOnlineDevices,
  parseActiveCalls,
  parseCallEvent,
  parseDevices,
  parseHandoff,
  parseSuggestion,
  pendingActive,
  settleMove,
  type PendingMove,
  handoffBody,
  parseContextResult,
  proximityBody,
  withSipInstance,
  startedAtMs,
  type ActiveElsewhereCall,
  type ContextKind,
} from "./callMove";

const wire = (over: Record<string, unknown> = {}) => ({
  id: "c1",
  state: "active",
  direction: "inbound",
  remote: { number: "+13035550100", name: null },
  started_at: "2026-11-25T22:00:00Z",
  device: { registration_id: "r-ios", label: "Aria iOS", user_agent: "ua", transport: "tls" },
  sip_call_id: "leg-1",
  ...over,
});

describe("calls elsewhere", () => {
  it("drops a call with no registration when this app has a call to the same number", () => {
    const [unnamed] = parseActiveCalls([wire({ device: { registration_id: null, label: "x" }, sip_call_id: "other-leg" })]);
    expect(callsElsewhere([unnamed], ["mine"], null, ["13035550100"])).toEqual([]);
    expect(callsElsewhere([unnamed], ["mine"], null, ["+14045550111"])).toEqual([unnamed]);
    expect(callsElsewhere([unnamed], ["mine"], null, [])).toEqual([unnamed]);
  });
});

describe("active calls", () => {
  it("keeps a call on a device the PBX could not name, so the banner still shows", () => {
    const [c] = parseActiveCalls([wire({ device: { registration_id: null, label: "Aria for Android" } })]);
    expect(c).toMatchObject({ id: "c1", device: { registrationId: "", label: "Aria for Android" } });
  });

  it("parses the PBX's objects and drops malformed ones", () => {
    const calls = parseActiveCalls([wire(), { id: "x" }, null, wire({ id: "c2", state: "held", remote: { number: "101", name: "Ann" } })]);
    expect(calls.map((c) => c.id)).toEqual(["c1", "c2"]);
    expect(calls[1]).toMatchObject({ state: "held", remote: { number: "101", name: "Ann" } });
    expect(calls[0].device).toEqual({ registrationId: "r-ios", label: "Aria iOS" });
    expect(parseActiveCalls("nope")).toEqual([]);
  });

  it("ignores a call whose leg is this app's own dialog", () => {
    const calls = parseActiveCalls([wire(), wire({ id: "c2", sip_call_id: "mine" })]);
    expect(callsElsewhere(calls, ["mine"], null).map((c) => c.id)).toEqual(["c1"]);
  });

  it("ignores a call on this device's registration", () => {
    const calls = parseActiveCalls([wire()]);
    expect(callsElsewhere(calls, [], "r-ios")).toEqual([]);
  });

  it("carries the original start so the timer continues", () => {
    const [c] = parseActiveCalls([wire()]);
    expect(startedAtMs(c)).toBe(Date.parse("2026-11-25T22:00:00Z"));
    expect(startedAtMs({ startedAt: "garbage" })).toBeNull();
  });
});

describe("events", () => {
  const [call] = parseActiveCalls([wire()]) as ActiveElsewhereCall[];

  it("parses the three call events and the suggestion", () => {
    expect(parseCallEvent({ type: "call_active_elsewhere", call: wire() })).toMatchObject({ type: "call_active_elsewhere" });
    expect(parseCallEvent({ type: "call_moved", call_id: "c1", from_registration_id: "a", to_registration_id: "b" })).toEqual({
      type: "call_moved",
      callId: "c1",
      fromRegistrationId: "a",
      toRegistrationId: "b",
    });
    expect(parseCallEvent({ type: "call_ended", call_id: "c1" })).toEqual({ type: "call_ended", callId: "c1" });
    expect(parseCallEvent({ type: "handoff_suggested" })?.type).toBe("handoff_suggested");
    expect(parseCallEvent({ type: "sms_received" })).toBeNull();
    expect(parseCallEvent({ type: "call_active_elsewhere", call: { id: "" } })).toBeNull();
  });

  it("adds, replaces and removes calls", () => {
    let calls = applyToCalls([], { type: "call_active_elsewhere", call });
    calls = applyToCalls(calls, { type: "call_active_elsewhere", call: { ...call, state: "held" } });
    expect(calls).toHaveLength(1);
    expect(calls[0].state).toBe("held");
    expect(applyToCalls(calls, { type: "call_ended", callId: "c1" })).toEqual([]);
    expect(applyToCalls(calls, { type: "call_moved", callId: "c1", fromRegistrationId: "a", toRegistrationId: "b" })).toEqual([]);
  });
});

describe("moving", () => {
  it("expires a pending move after 30 seconds", () => {
    const p = { moveId: "m", callId: "c", startedAt: 1, expiresAt: 1000 + PENDING_MOVE_MS, toThisDevice: true };
    expect(pendingActive(p, 1000 + PENDING_MOVE_MS - 1)).toBe(p);
    expect(pendingActive(p, 1000 + PENDING_MOVE_MS)).toBeNull();
    expect(pendingActive(null, 0)).toBeNull();
  });

  it("names each refusal", () => {
    expect(moveFailureKey(404)).toBe("callMove.errNotFound");
    expect(moveFailureKey(409)).toBe("callMove.errBusy");
    expect(moveFailureKey(422)).toBe("callMove.errDevice");
    expect(moveFailureKey(500)).toBe("callMove.errGeneric");
    expect(moveFailureKey(undefined)).toBe("callMove.errGeneric");
  });

  it("offers other online devices only", () => {
    const devices = parseDevices([
      { registration_id: "me", label: "This Mac", online: true },
      { registration_id: "a", label: "Aria iOS", online: true },
      { registration_id: "b", label: "Desk", online: false },
    ]);
    expect(otherOnlineDevices(devices, "me").map((d) => d.label)).toEqual(["Aria iOS"]);
  });

  it("calls a BYE after call_moved a move, in either order, and only soon after", () => {
    expect(movedAway({ label: "Aria iOS", at: 1000, sipCallId: null }, 2000, "leg")).toBe("Aria iOS");
    expect(movedAway({ label: "", at: 1000, sipCallId: null }, 2000, undefined)).toBe("");
    expect(movedAway({ label: "x", at: 1000, sipCallId: null }, 1000 + 60_000, "leg")).toBeNull();
    expect(movedAway(null, 5, "leg")).toBeNull();
  });
});

describe("context", () => {
  const none = new Set<ContextKind>();
  it("reports a lock, then idle, once each", () => {
    expect(contextToReport({ locked: true, idleSecs: 0 }, none)).toBe("desktop_locked");
    expect(contextToReport({ locked: true, idleSecs: 999 }, new Set<ContextKind>(["desktop_locked"]))).toBeNull();
    expect(contextToReport({ locked: false, idleSecs: 180 }, none)).toBe("desktop_idle");
    expect(contextToReport({ locked: false, idleSecs: 179 }, none)).toBeNull();
    expect(contextToReport({ locked: false, idleSecs: 500 }, new Set<ContextKind>(["desktop_idle"]))).toBeNull();
  });
});

describe("handoff", () => {
  it("reads settings with the server's names and defaults", () => {
    expect(parseHandoff({ suggest_nearby: false, auto_move_on_desktop_lock: true, ble_opt_in: true })).toEqual({
      suggestNearby: false,
      autoMoveOnLock: true,
      bleOptIn: true,
    });
    expect(parseHandoff({})).toEqual({ suggestNearby: true, autoMoveOnLock: false, bleOptIn: false });
    expect(handoffBody({ suggestNearby: true, autoMoveOnLock: true, bleOptIn: false })).toEqual({
      suggest_nearby: true,
      auto_move_on_desktop_lock: true,
      ble_opt_in: false,
    });
  });

  it("reads a handoff_suggested event and names the device from the list", () => {
    const devices = parseDevices([{ registration_id: "a", label: "Aria iOS", online: true }]);
    const s = parseSuggestion({ type: "handoff_suggested", call_id: "c1", to_registration_id: "a", reason: "nearby" }, devices);
    expect(s?.device.label).toBe("Aria iOS");
    expect(s?.callId).toBe("c1");
    expect(s?.reason).toBe("nearby");
    expect(parseSuggestion({ call_id: "c1" }, devices)).toBeNull();
  });

  it("names the device in the query and sends the proximity as a hash only", () => {
    expect(withSipInstance("/me/devices/current/proximity", "urn:uuid:a")).toBe("/me/devices/current/proximity?sip_instance=urn%3Auuid%3Aa");
    expect(proximityBody("ab".repeat(32))).toEqual({ network_hash: "ab".repeat(32) });
  });

  it("reads the context answer", () => {
    expect(parseContextResult({ action: "moved", call_id: "c", move_id: "m" })).toEqual({ action: "moved", callId: "c", moveId: "m" });
    expect(parseContextResult({ action: "none", call_id: null, move_id: null })).toEqual({ action: "none", callId: null, moveId: null });
  });

  it("movedAway matches the leg that moved, or any leg when the moved leg is unknown", () => {
    const at = 1_000_000;
    expect(movedAway({ label: "Aria iOS", at, sipCallId: "leg-1" }, at + 100, "leg-1")).toBe("Aria iOS");
    expect(movedAway({ label: "Aria iOS", at, sipCallId: "leg-1" }, at + 100, "leg-2")).toBeNull();
    expect(movedAway({ label: "Aria iOS", at, sipCallId: null }, at + 100, "leg-2")).toBe("Aria iOS");
    expect(movedAway({ label: "Aria iOS", at, sipCallId: "leg-1" }, at + 20_000, "leg-1")).toBeNull();
  });
});

describe("settleMove", () => {
  const p: PendingMove = { moveId: "m1", callId: "c1", startedAt: null, expiresAt: 30_000, toThisDevice: true };
  it("poll: moved and failed end it, pending and ringing do not", () => {
    expect(settleMove(p, { kind: "poll", status: "moved", detail: "" }, 1)).toEqual({ kind: "moved" });
    expect(settleMove(p, { kind: "poll", status: "failed", detail: "busy" }, 1)).toEqual({ kind: "failed", detail: "busy" });
    expect(settleMove(p, { kind: "poll", status: "pending", detail: "" }, 1)).toBeNull();
    expect(settleMove(p, { kind: "poll", status: "ringing", detail: "" }, 1)).toBeNull();
  });
  it("WS failure matches its move id only", () => {
    expect(settleMove(p, { kind: "failed_event", moveId: "M1", callId: "c1", detail: "d" }, 1)).toEqual({ kind: "failed", detail: "d" });
    expect(settleMove(p, { kind: "failed_event", moveId: "m2", callId: "c1", detail: "d" }, 1)).toBeNull();
  });
  it("call_ended and call_moved match the call", () => {
    expect(settleMove(p, { kind: "call_ended", callId: "c1" }, 1)).toEqual({ kind: "ended" });
    expect(settleMove(p, { kind: "call_ended", callId: "c2" }, 1)).toBeNull();
    expect(settleMove(p, { kind: "moved_event", callId: "c1" }, 1)).toEqual({ kind: "moved" });
  });
  it("times out at the limit on any signal; nothing pending is nothing", () => {
    expect(settleMove(p, { kind: "tick" }, 29_999)).toBeNull();
    expect(settleMove(p, { kind: "tick" }, 30_000)).toEqual({ kind: "timeout", detail: "" });
    expect(settleMove(null, { kind: "tick" }, 99_999)).toBeNull();
  });
  it("parses call_move_failed", () => {
    expect(parseCallEvent({ type: "call_move_failed", call_id: "c", move_id: "m", detail: "x" })).toEqual({ type: "call_move_failed", callId: "c", moveId: "m", detail: "x" });
  });
});
