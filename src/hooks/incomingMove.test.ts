import { beforeEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { matchMedia: () => ({ matches: false, addEventListener: () => {}, removeEventListener: () => {} }), addEventListener: () => {}, removeEventListener: () => {} },
  });
  const mem = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v), removeItem: (k: string) => void mem.delete(k), clear: () => mem.clear() },
  });
});
const invoke = vi.hoisted(() => vi.fn(async () => undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("../utils/pbxBinding", () => ({ ensureBound: vi.fn(async () => {}) }));
vi.mock("../i18n", () => ({ default: { t: (k: string) => k } }));

import { incomingMove } from "./useSip";
import { useCallMoveStore } from "../stores/callMoveStore";

const payload = (over: Record<string, unknown>) => ({ remoteUri: "sip:+13035550100@x", ...over }) as never;

describe("incomingMove", () => {
  beforeEach(() => {
    useCallMoveStore.getState().clearPending();
    useCallMoveStore.setState({ accountId: "acct", calls: [] });
  });

  it("auto-answers a move this app asked for and ends its pending state", () => {
    useCallMoveStore.getState().beginPending("c1", "M-1", true, 1234);
    const r = incomingMove(payload({ moveId: "M-1", moveAutoAnswer: true }));
    expect(r).toMatchObject({ moveId: "M-1", auto: true, startTime: 1234 });
    expect(useCallMoveStore.getState().pending).toBeNull();
  });

  it("an answer for another move leaves the pending one alone", () => {
    useCallMoveStore.getState().beginPending("c1", "M-1", true, null);
    incomingMove(payload({ moveId: "M-2", moveAutoAnswer: true }));
    expect(useCallMoveStore.getState().pending?.moveId).toBe("m-1");
  });

  it("rings a move someone else asked for", () => {
    useCallMoveStore.getState().beginPending("c1", "M-1", true, null);
    const r = incomingMove(payload({ moveId: "M-3" }));
    expect(r.auto).toBe(false);
    expect(useCallMoveStore.getState().pending?.moveId).toBe("m-1");
  });
});
