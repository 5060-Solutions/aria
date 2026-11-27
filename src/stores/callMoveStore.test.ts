import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("../utils/pbxBinding", () => ({ ensureBound: vi.fn(async () => {}) }));
vi.mock("../i18n", () => ({ default: { t: (k: string, o?: { device?: string }) => (o?.device ? `${k}:${o.device}` : k) } }));

vi.mock("./appStore", async () => {
  const { create } = await import("zustand");
  return { useAppStore: create(() => ({ activeCalls: [] as { id: string; state: string }[] })) };
});

import { useCallMoveStore } from "./callMoveStore";
import { useToastStore } from "./toastStore";
import { useAppStore } from "./appStore";
import { parseActiveCalls } from "../utils/callMove";

const mailbox = { accountId: "acct", apiUrl: "https://api.example.com", extension: "1001", domain: "t.example" };
const wire = {
  id: "11111111-1111-4111-8111-111111111111",
  state: "active",
  direction: "inbound",
  remote: { number: "+13035550100", name: "Ann" },
  started_at: "2026-11-25T22:00:00Z",
  device: { registration_id: "r-ios", label: "Aria iOS", user_agent: "", transport: "tls" },
  sip_call_id: "leg-1",
};

/** The PBX, mocked: what `pbx_me_request` answers for each route. */
function pbx(routes: Record<string, unknown | Error>) {
  invoke.mockImplementation(async (cmd: string, args?: { method?: string; path?: string }) => {
    if (cmd === "sip_instance_urn") return "urn:uuid:abc";
    if (cmd === "callmove_expect" || cmd === "callmove_request_started" || cmd === "callmove_request_finished" || cmd === "callmove_forget") return undefined;
    if (cmd !== "pbx_me_request") throw new Error(`unexpected ${cmd}`);
    const hit = routes[`${args?.method} ${args?.path}`];
    if (hit === undefined) throw new Error(`unrouted ${args?.method} ${args?.path}`);
    if (hit instanceof Error) throw hit;
    return hit;
  });
}

const store = () => useCallMoveStore.getState();

describe("callMoveStore", () => {
  beforeEach(() => {
    invoke.mockReset();
    useCallMoveStore.setState({ accountId: "acct", calls: [], devices: [], current: null, currentAt: 0, requesting: false, pending: null, movedTo: null, suggestion: null });
    useToastStore.setState({ current: null, queue: [] });
    useAppStore.setState({ activeCalls: [] });
  });

  afterEach(() => {
    store().clearPending();
    vi.useRealTimers();
  });

  it("polls the active calls", async () => {
    pbx({ "GET /me/calls/active": [wire] });
    await store().refreshCalls(mailbox);
    expect(store().calls.map((c) => c.sipCallId)).toEqual(["leg-1"]);
  });

  it("moves here: finds its own registration by +sip.instance, posts the move, keeps the move_id", async () => {
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
      "POST /me/calls/11111111-1111-4111-8111-111111111111/move": { move_id: "M-1" },
    });
    const [call] = parseActiveCalls([wire]);
    expect(await store().moveHere(mailbox, call)).toBe(true);
    expect(invoke).toHaveBeenCalledWith("pbx_me_request", {
      mailbox,
      method: "POST",
      path: "/me/calls/11111111-1111-4111-8111-111111111111/move",
      body: { to_registration_id: "r-mac" },
    });
    expect(invoke).toHaveBeenCalledWith("callmove_expect", { accountId: "acct", moveId: "M-1" });
    const p = store().pending;
    expect(p).toMatchObject({ moveId: "m-1", startedAt: Date.parse("2026-11-25T22:00:00Z") });
    expect(store().pendingFor("M-1")).toBe(p);
    expect(store().pendingFor("other")).toBeNull();
  });

  it("does not auto-answer a move to another device", async () => {
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
      "POST /me/calls/11111111-1111-4111-8111-111111111111/move": { move_id: "M-2" },
    });
    const [call] = parseActiveCalls([wire]);
    await store().ensureCurrent(mailbox);
    expect(await store().move(mailbox, call, "r-ios")).toBe(true);
    expect(store().pending).toMatchObject({ moveId: "m-2", toThisDevice: false });
    expect(invoke).not.toHaveBeenCalledWith("callmove_expect", expect.anything());
  });

  it("tells the person why a move was refused and keeps nothing pending", async () => {
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
      "POST /me/calls/11111111-1111-4111-8111-111111111111/move": Object.assign(new Error("x"), { status: 409 }),
    });
    const [call] = parseActiveCalls([wire]);
    expect(await store().moveHere(mailbox, call)).toBe(false);
    expect(store().pending).toBeNull();
    expect(useToastStore.getState().current?.message).toBe("callMove.errBusy");
  });

  it("applies WebSocket events to the list", () => {
    const [call] = parseActiveCalls([wire]);
    store().applyEvent({ type: "call_active_elsewhere", call }, null);
    expect(store().calls).toHaveLength(1);
    store().applyEvent({ type: "call_ended", callId: call.id }, null);
    expect(store().calls).toEqual([]);
  });

  it("says Moved to <device> at once when the BYE came first", () => {
    useCallMoveStore.setState({ current: "r-mac", devices: [{ registrationId: "r-ios", label: "Aria iOS", online: true }] });
    useAppStore.setState({
      activeCalls: [{ id: "x", state: "ended" } as never],
    });
    store().applyEvent({ type: "call_moved", callId: "c", fromRegistrationId: "r-mac", toRegistrationId: "r-ios" }, null);
    expect(useToastStore.getState().current?.message).toBe("callMove.movedTo:Aria iOS");
    expect(store().movedTo).toBeNull();
  });

  it("remembers the move for the BYE that follows while the call is still up", () => {
    useCallMoveStore.setState({ current: "r-mac", devices: [{ registrationId: "r-ios", label: "Aria iOS", online: true }] });
    useAppStore.setState({ activeCalls: [{ id: "x", state: "connected" } as never] });
    store().applyEvent({ type: "call_moved", callId: "c", fromRegistrationId: "r-mac", toRegistrationId: "r-ios" }, null);
    expect(store().movedTo?.label).toBe("Aria iOS");
    expect(useToastStore.getState().current).toBeNull();
  });

  it("registers a move the context report started here as pending", async () => {
    const path = "/me/devices/current/context?sip_instance=urn%3Auuid%3Aabc";
    pbx({ [`POST ${path}`]: { action: "moved", call_id: "c1", move_id: "M-2" } });
    const res = await store().reportContext(mailbox, "desktop_locked");
    expect(res.action).toBe("moved");
    expect(invoke).toHaveBeenCalledWith("pbx_me_request", { mailbox, method: "POST", path, body: { event: "desktop_locked" } });
    expect(invoke).toHaveBeenCalledWith("callmove_expect", { accountId: "acct", moveId: "M-2" });
    expect(store().pendingFor("m-2")?.callId).toBe("c1");
    pbx({ [`POST ${path}`]: { action: "suggested", call_id: "c1", move_id: null } });
    store().clearPending();
    await store().reportContext(mailbox, "desktop_idle");
    expect(store().pending).toBeNull();
  });

  it("holds the INVITE for the 202: a request is in flight until the move id is registered", async () => {
    const order: string[] = [];
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
      "POST /me/calls/11111111-1111-4111-8111-111111111111/move": { move_id: "M-3" },
    });
    const base = invoke.getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    invoke.mockImplementation(async (cmd: string, args?: unknown) => {
      order.push(cmd);
      return base(cmd, args);
    });
    const [call] = parseActiveCalls([wire]);
    await store().moveHere(mailbox, call);
    expect(order.filter((c) => c.startsWith("callmove"))).toEqual(["callmove_request_started", "callmove_expect", "callmove_request_finished"]);
  });

  it("releases the held INVITE when the move request fails", async () => {
    const order: string[] = [];
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
      "POST /me/calls/11111111-1111-4111-8111-111111111111/move": new Error("down"),
    });
    const base = invoke.getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    invoke.mockImplementation(async (cmd: string, args?: unknown) => {
      order.push(cmd);
      return base(cmd, args);
    });
    const [call] = parseActiveCalls([wire]);
    expect(await store().moveHere(mailbox, call)).toBe(false);
    expect(order.filter((c) => c.startsWith("callmove"))).toEqual(["callmove_request_started", "callmove_request_finished"]);
  });

  it("ignores a move between two other devices", () => {
    useCallMoveStore.setState({ current: "r-mac" });
    store().applyEvent({ type: "call_moved", callId: "c", fromRegistrationId: "r-a", toRegistrationId: "r-b" }, null);
    expect(store().movedTo).toBeNull();
  });

  it("keeps the saved settings and rolls back on a refusal", async () => {
    pbx({ "PUT /me/call-handoff": null });
    await store().saveHandoff(mailbox, { suggestNearby: true, autoMoveOnLock: false, bleOptIn: false });
    expect(store().handoff.suggestNearby).toBe(true);
    pbx({ "PUT /me/call-handoff": new Error("down") });
    await store().saveHandoff(mailbox, { suggestNearby: false, autoMoveOnLock: true, bleOptIn: false });
    expect(store().handoff.suggestNearby).toBe(true);
    expect(store().handoff.autoMoveOnLock).toBe(false);
  });

  it("shows a suggestion from handoff_suggested", () => {
    useCallMoveStore.setState({ devices: [{ registrationId: "r-ios", label: "Aria iOS", online: true }] });
    store().applyEvent({ type: "handoff_suggested", raw: { call_id: "c1", to_registration_id: "r-ios", reason: "nearby" } }, null);
    expect(store().suggestion?.device.label).toBe("Aria iOS");
  });

  it("forgets one account's state when another becomes active", () => {
    useCallMoveStore.setState({ accountId: "acct", calls: parseActiveCalls([wire]), current: "r-mac", pending: { moveId: "m", callId: "c", startedAt: null, expiresAt: Date.now() + 1000, toThisDevice: true } });
    store().forAccount("acct");
    expect(store().current).toBe("r-mac");
    store().forAccount("acct-2");
    expect(store().accountId).toBe("acct-2");
    expect(store().calls).toEqual([]);
    expect(store().current).toBeNull();
    expect(store().pending).toBeNull();
  });

  it("re-reads its own registration id before a move, so a re-register or account switch cannot use a stale one", async () => {
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
      "POST /me/calls/11111111-1111-4111-8111-111111111111/move": { move_id: "M-9" },
    });
    useCallMoveStore.setState({ current: "r-stale", currentAt: 0 });
    const [call] = parseActiveCalls([wire]);
    expect(await store().moveHere(mailbox, call)).toBe(true);
    const posted = invoke.mock.calls.find((c) => c[0] === "pbx_me_request" && (c[1] as { method: string }).method === "POST");
    expect((posted?.[1] as { body: { to_registration_id: string } }).body.to_registration_id).toBe("r-mac");
    expect(store().pendingFor("m-9")).not.toBeNull();
  });

  it("has the pending move in place before the backend releases the held INVITE", async () => {
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
      "POST /me/calls/11111111-1111-4111-8111-111111111111/move": { move_id: "M-4" },
    });
    const base = invoke.getMockImplementation() as (cmd: string, args?: unknown) => Promise<unknown>;
    let pendingAtExpect: unknown = "unset";
    invoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "callmove_expect") pendingAtExpect = store().pending?.moveId ?? null;
      return base(cmd, args);
    });
    const [call] = parseActiveCalls([wire]);
    await store().moveHere(mailbox, call);
    expect(pendingAtExpect).toBe("m-4");
  });

  it("refuses a second move request while one is still being posted", async () => {
    let release: (v: unknown) => void = () => {};
    const slow = new Promise((r) => (release = r));
    pbx({
      "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc": { registration_id: "r-mac" },
    });
    const base = invoke.getMockImplementation() as (cmd: string, args?: { method?: string }) => Promise<unknown>;
    invoke.mockImplementation(async (cmd: string, args?: { method?: string }) => {
      if (cmd === "pbx_me_request" && args?.method === "POST") return slow.then(() => ({ move_id: "M-5" }));
      return base(cmd, args);
    });
    const [call] = parseActiveCalls([wire]);
    const first = store().moveHere(mailbox, call);
    await new Promise((r) => setTimeout(r, 0));
    expect(store().requesting).toBe(true);
    expect(await store().move(mailbox, call, "r-ios")).toBe(false);
    release(null);
    expect(await first).toBe(true);
    expect(store().requesting).toBe(false);
    expect(invoke.mock.calls.filter((c) => c[0] === "pbx_me_request" && (c[1] as { method: string }).method === "POST")).toHaveLength(1);
  });

  it("ties Moved to <device> to the leg that moved, and says it once that leg is gone", () => {
    const [call] = parseActiveCalls([wire]);
    useCallMoveStore.setState({ calls: [call], current: "r-mac", devices: [{ registrationId: "r-ios", label: "Aria iOS", online: true }] });
    // Another call is still up here: the moved one (leg-1) is already gone.
    useAppStore.setState({ activeCalls: [{ id: "y", state: "connected", sipCallId: "leg-2" } as never] });
    store().applyEvent({ type: "call_moved", callId: call.id, fromRegistrationId: "r-mac", toRegistrationId: "r-ios" }, null);
    expect(useToastStore.getState().current?.message).toBe("callMove.movedTo:Aria iOS");
    expect(store().movedTo).toBeNull();
  });

  it("keeps Moved to for the leg still up, with its Call-ID", () => {
    const [call] = parseActiveCalls([wire]);
    useCallMoveStore.setState({ calls: [call], current: "r-mac", devices: [{ registrationId: "r-ios", label: "Aria iOS", online: true }] });
    useAppStore.setState({ activeCalls: [{ id: "x", state: "connected", sipCallId: "leg-1" } as never] });
    store().applyEvent({ type: "call_moved", callId: call.id, fromRegistrationId: "r-mac", toRegistrationId: "r-ios" }, null);
    expect(store().movedTo).toMatchObject({ label: "Aria iOS", sipCallId: "leg-1" });
  });

  it("drops a suggestion once its call ends or moves", () => {
    useCallMoveStore.setState({ devices: [{ registrationId: "r-ios", label: "Aria iOS", online: true }] });
    store().applyEvent({ type: "handoff_suggested", raw: { call_id: "c1", to_registration_id: "r-ios", reason: "nearby" } }, null);
    expect(store().suggestion).not.toBeNull();
    store().applyEvent({ type: "call_ended", callId: "c1" }, null);
    expect(store().suggestion).toBeNull();
    store().applyEvent({ type: "handoff_suggested", raw: { call_id: "c2", to_registration_id: "r-ios", reason: "nearby" } }, null);
    store().applyEvent({ type: "call_moved", callId: "c2", fromRegistrationId: "r-a", toRegistrationId: "r-b" }, null);
    expect(store().suggestion).toBeNull();
  });

  describe("a pending move never hangs", () => {
    const callId = "11111111-1111-4111-8111-111111111111";
    const movePath = `POST /me/calls/${callId}/move`;
    const cur = "GET /me/devices/current?sip_instance=urn%3Auuid%3Aabc";
    const statusPath = "GET /me/moves/m-9";

    async function start(extra: Record<string, unknown | Error> = {}) {
      pbx({ [cur]: { registration_id: "r-mac" }, [movePath]: { move_id: "M-9" }, ...extra });
      const [call] = parseActiveCalls([wire]);
      expect(await store().moveHere(mailbox, call)).toBe(true);
      expect(store().pending?.moveId).toBe("m-9");
    }
    const msg = () => useToastStore.getState().current?.message;

    it.each([
      [404, "callMove.errNotFound"],
      [409, "callMove.errBusy"],
      [422, "callMove.errDevice"],
    ])("a %i from the POST shows its message and is never pending", async (status, key) => {
      pbx({ [cur]: { registration_id: "r-mac" }, [movePath]: Object.assign(new Error("x"), { status }) });
      const [call] = parseActiveCalls([wire]);
      expect(await store().moveHere(mailbox, call)).toBe(false);
      expect(store().pending).toBeNull();
      expect(msg()).toBe(key);
      expect(invoke).not.toHaveBeenCalledWith("callmove_expect", expect.anything());
    });

    const statusCalls = () => invoke.mock.calls.filter(([c, a]) => c === "pbx_me_request" && (a as { path: string }).path === "/me/moves/m-9");
    const forgets = () => invoke.mock.calls.filter(([c]) => c === "callmove_forget");

    it("failed via the status poll", async () => {
      vi.useFakeTimers();
      await start({ [statusPath]: { status: "failed", detail: "no answer" } });
      await vi.advanceTimersByTimeAsync(2_000);
      expect(store().pending).toBeNull();
      expect(msg()).toBe("callMove.failed");
      // The backend is told to stop expecting it, and nothing polls on.
      expect(forgets()).toEqual([["callmove_forget", { accountId: "acct", moveId: "m-9" }]]);
      const n = statusCalls().length;
      await vi.advanceTimersByTimeAsync(60_000);
      expect(statusCalls()).toHaveLength(n);
      expect(vi.getTimerCount()).toBe(0);
    });

    it("keeps polling while pending or ringing", async () => {
      vi.useFakeTimers();
      await start({ [statusPath]: { status: "ringing" } });
      await vi.advanceTimersByTimeAsync(6_000);
      expect(statusCalls()).toHaveLength(3);
      expect(store().pending).not.toBeNull();
      expect(msg()).toBeUndefined();
      pbx({ [statusPath]: { status: "pending" }, [cur]: { registration_id: "r-mac" } });
      await vi.advanceTimersByTimeAsync(2_000);
      expect(statusCalls()).toHaveLength(4);
    });

    it("moved via the status poll clears quietly", async () => {
      vi.useFakeTimers();
      await start({ [statusPath]: { status: "moved" } });
      await vi.advanceTimersByTimeAsync(2_000);
      expect(store().pending).toBeNull();
      expect(msg()).toBeUndefined();
    });

    it("failed via the WebSocket event", async () => {
      await start();
      store().applyEvent({ type: "call_move_failed", callId, moveId: "M-9", detail: "declined" }, null);
      expect(store().pending).toBeNull();
      expect(msg()).toBe("callMove.failed");
    });

    it("ignores a failure for another move", async () => {
      await start();
      store().applyEvent({ type: "call_move_failed", callId: "other", moveId: "M-other", detail: "x" }, null);
      expect(store().pending).not.toBeNull();
    });

    it("times out after 30 s, and poll errors do not extend it", async () => {
      vi.useFakeTimers();
      await start({ [statusPath]: new Error("network") });
      expect(invoke).toHaveBeenCalledWith("callmove_expect", { accountId: "acct", moveId: "M-9" });
      await vi.advanceTimersByTimeAsync(29_000);
      expect(store().pending).not.toBeNull();
      await vi.advanceTimersByTimeAsync(1_000);
      expect(store().pending).toBeNull();
      expect(msg()).toBe("callMove.failedNoDetail");
      expect(forgets()).toHaveLength(1);
    });

    it("moved via call_moved", async () => {
      await start();
      store().applyEvent({ type: "call_moved", callId, fromRegistrationId: "r-ios", toRegistrationId: "r-mac" }, null);
      expect(store().pending).toBeNull();
      expect(msg()).toBeUndefined();
    });

    it("call_ended ends the pending move quietly", async () => {
      await start();
      store().applyEvent({ type: "call_ended", callId }, null);
      expect(store().pending).toBeNull();
      expect(msg()).toBeUndefined();
    });

    it("the answered INVITE clears it", async () => {
      await start();
      store().settle({ kind: "answered", moveId: "other" });
      expect(store().pending).not.toBeNull();
      store().settle({ kind: "answered", moveId: "M-9" });
      expect(store().pending).toBeNull();
    });

    it("a second move replaces the first and revokes it", async () => {
      vi.useFakeTimers();
      await start({ [statusPath]: { status: "pending" } });
      invoke.mockClear();
      pbx({ [cur]: { registration_id: "r-mac" }, [movePath]: { move_id: "M-10" }, "GET /me/moves/m-10": { status: "pending" }, [statusPath]: { status: "pending" } });
      const [call] = parseActiveCalls([wire]);
      expect(await store().moveHere(mailbox, call)).toBe(true);
      expect(store().pending?.moveId).toBe("m-10");
      expect(forgets()).toContainEqual(["callmove_forget", { accountId: "acct", moveId: "m-9" }]);
      await vi.advanceTimersByTimeAsync(2_000);
      expect(statusCalls()).toHaveLength(0);
    });

    it("a poll reply for the old move is ignored", async () => {
      vi.useFakeTimers();
      let release: (v: unknown) => void = () => {};
      await start();
      invoke.mockImplementation(async (cmd: string, args?: { path?: string; method?: string }) => {
        if (cmd === "pbx_me_request" && args?.path === "/me/moves/m-9") return new Promise((r) => (release = r));
        return undefined;
      });
      await vi.advanceTimersByTimeAsync(2_000);
      // The poll is in flight; a new move takes over before it answers.
      store().trackMove(mailbox, "c2", "M-11");
      release({ status: "failed", detail: "old" });
      await vi.advanceTimersByTimeAsync(0);
      expect(store().pending?.moveId).toBe("m-11");
      expect(msg()).toBeUndefined();
    });

    it("polls do not stack while one request is slow", async () => {
      vi.useFakeTimers();
      await start();
      let calls = 0;
      invoke.mockImplementation(async (cmd: string) => {
        if (cmd === "pbx_me_request") {
          calls++;
          return new Promise(() => {});
        }
        return undefined;
      });
      await vi.advanceTimersByTimeAsync(20_000);
      expect(calls).toBe(1);
    });

    it("a failed Move to... on another device is reported too", async () => {
      vi.useFakeTimers();
      pbx({
        [cur]: { registration_id: "r-mac" },
        [movePath]: { move_id: "M-9" },
        [statusPath]: { status: "failed", detail: "device offline" },
      });
      const [call] = parseActiveCalls([wire]);
      await store().ensureCurrent(mailbox);
      expect(await store().move(mailbox, call, "r-ios")).toBe(true);
      expect(store().pending).toMatchObject({ moveId: "m-9", toThisDevice: false });
      expect(invoke).not.toHaveBeenCalledWith("callmove_expect", expect.anything());
      await vi.advanceTimersByTimeAsync(2_000);
      expect(store().pending).toBeNull();
      expect(msg()).toBe("callMove.failed");
    });
  });
});
