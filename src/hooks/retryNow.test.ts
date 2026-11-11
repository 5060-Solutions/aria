import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountState, RegistrationState, SipAccount } from "../types/sip";
import type { ConnectionReport } from "../types/connection";

// The store and the hook module both touch browser globals when they load,
// so stub those and the Tauri bridge before importing either.
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

function memoryStorage(): Storage {
  const data = new Map<string, string>();
  return {
    get length() {
      return data.size;
    },
    clear: () => data.clear(),
    getItem: (k: string) => data.get(k) ?? null,
    key: (i: number) => [...data.keys()][i] ?? null,
    removeItem: (k: string) => void data.delete(k),
    setItem: (k: string, v: string) => void data.set(k, v),
  };
}
vi.stubGlobal("localStorage", memoryStorage());
vi.stubGlobal("window", {
  matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
  addEventListener() {},
  removeEventListener() {},
});

const { useAppStore } = await import("../stores/appStore");
const { cancelRetry, retryNow, retryRegister } = await import("./useSip");

function acct(id: string): SipAccount {
  return {
    id,
    displayName: id,
    username: "1001",
    domain: "pbx.example.com",
    password: "",
    transport: "udp",
    port: 5060,
    enabled: true,
  };
}

function st(state: RegistrationState, error: string | null): AccountState {
  return { accountId: "a1", registrationState: state, registrationError: error };
}

function report(accountId: string): ConnectionReport {
  return {
    accountId,
    username: "1001",
    domain: "pbx.example.com",
    transport: "udp",
    target: { host: "pbx.example.com", port: 5060, viaProxy: false },
    steps: [],
    outcome: "error",
    error: "no reply",
    reconnectAttempt: 0,
    registered: null,
    attemptStartedMs: 0,
    attemptFinishedMs: null,
  };
}

describe("retryNow", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invoke.mockReset();
    invoke.mockResolvedValue("registering");
    useAppStore.setState({
      accounts: [acct("a1")],
      activeAccountId: "a1",
      accountStates: { a1: st("error", "no reply") },
      retryPlans: {},
      connectionReports: {},
    });
  });

  afterEach(() => {
    cancelRetry("a1");
    vi.useRealTimers();
  });

  it("records the scheduled retry while retryRegister sleeps", async () => {
    const p = retryRegister("a1");
    await vi.advanceTimersByTimeAsync(0);
    const plan = useAppStore.getState().retryPlans.a1;
    expect(plan.kind).toBe("scheduled");
    if (plan.kind === "scheduled") expect(plan.at).toBeGreaterThan(Date.now());
    cancelRetry("a1");
    await vi.runAllTimersAsync();
    await p;
  });

  it("cancels a pending retry and calls sip_register_now once", async () => {
    const p = retryRegister("a1");
    await vi.advanceTimersByTimeAsync(0);
    await retryNow("a1");
    await vi.runAllTimersAsync();
    await p;
    const calls = invoke.mock.calls.map((c) => c[0]);
    expect(calls.filter((c) => c === "sip_register_now")).toHaveLength(1);
    expect(calls.filter((c) => c === "sip_register")).toHaveLength(0);
    expect(useAppStore.getState().accountStates.a1.registrationState).toBe("registering");
    expect(useAppStore.getState().retryPlans.a1).toEqual({ kind: "none" });
  });

  it("re-arms the retry loop when the forced attempt fails", async () => {
    invoke.mockImplementation(async (cmd: string) =>
      cmd === "sip_register_now"
        ? Promise.reject(new Error("DNS resolve failed for x:5060: no such host"))
        : "",
    );
    await retryNow("a1");
    const s = useAppStore.getState();
    expect(s.accountStates.a1.registrationState).toBe("error");
    expect(s.accountStates.a1.registrationError).toContain("DNS resolve failed");
    expect(s.retryPlans.a1.kind).toBe("scheduled");
  });

  it("marks an auth failure as stopped and does not sleep", async () => {
    useAppStore.setState({
      accountStates: { a1: st("error", "401: Authentication failed - check username and password") },
    });
    await retryRegister("a1");
    expect(useAppStore.getState().retryPlans.a1).toEqual({ kind: "stopped", reason: "auth" });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("drops the report and plan when the account is removed", () => {
    useAppStore.getState().setConnectionReport(report("a1"));
    useAppStore.getState().setRetryPlan("a1", { kind: "none" });
    useAppStore.getState().removeAccount("a1");
    expect(useAppStore.getState().connectionReports.a1).toBeUndefined();
    expect(useAppStore.getState().retryPlans.a1).toBeUndefined();
  });
});
