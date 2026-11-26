import { beforeEach, describe, expect, it, vi } from "vitest";

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
    if (cmd === "callmove_expect" || cmd === "callmove_request_started" || cmd === "callmove_request_finished") return undefined;
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
    useCallMoveStore.setState({ calls: [], devices: [], current: null, pending: null, movedTo: null, suggestion: null });
    useToastStore.setState({ current: null, queue: [] });
    useAppStore.setState({ activeCalls: [] });
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
    expect(invoke).toHaveBeenCalledWith("callmove_expect", { moveId: "M-1" });
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
    expect(store().pending).toBeNull();
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
    expect(invoke).toHaveBeenCalledWith("callmove_expect", { moveId: "M-2" });
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
});
