import { describe, expect, it } from "vitest";
import {
  accountLine,
  copyDetails,
  failedStep,
  registeredLine,
  retryLine,
  stepLabelKey,
} from "./connectionReport";
import type { ConnectionReport, ConnectionStep, RetryPlan } from "../types/connection";

const t = (key: string, options?: Record<string, unknown>) =>
  options ? `${key}${JSON.stringify(options)}` : key;

function step(
  kind: ConnectionStep["kind"],
  status: ConnectionStep["status"],
  detail: string | null = null,
  elapsedMs: number | null = null,
): ConnectionStep {
  return { kind, status, detail, elapsedMs, failure: status === "failed" ? "timeout" : null };
}

function report(overrides: Partial<ConnectionReport> = {}): ConnectionReport {
  return {
    accountId: "a1",
    username: "ghostbook",
    domain: "harness.lan",
    transport: "tls",
    target: { host: "203.0.113.9", port: 5061, viaProxy: true },
    steps: [
      step("resolve", "skipped", "literal address"),
      step("connect", "ok", "handshake in 84 ms, certificate OK", 84),
      step("register", "failed", "no reply in 32 s (sent 7 times)", 32000),
      step("authenticate", "pending"),
      step("registered", "pending"),
    ],
    outcome: "error",
    error: "Registration timed out",
    reconnectAttempt: 0,
    registered: null,
    attemptStartedMs: 0,
    attemptFinishedMs: null,
    ...overrides,
  };
}

describe("connection report helpers", () => {
  it("finds the failed step", () => {
    expect(failedStep(report())?.kind).toBe("register");
    expect(failedStep(report({ steps: [step("resolve", "ok")] }))).toBeNull();
  });

  it("builds the account line with and without a proxy", () => {
    expect(accountLine(report())).toBe("ghostbook@harness.lan · TLS · proxy 203.0.113.9:5061");
    expect(
      accountLine(report({ transport: "udp", target: { host: "pbx.lan", port: 5060, viaProxy: false } })),
    ).toBe("ghostbook@harness.lan · UDP · pbx.lan:5060");
  });

  it("names step labels by kind", () => {
    expect(stepLabelKey("authenticate")).toBe("connection.steps.authenticate");
  });

  it("counts down to the scheduled retry and never goes negative", () => {
    const now = 1_000_000;
    const soon: RetryPlan = { kind: "scheduled", at: now + 2400 };
    expect(retryLine(report(), soon, now, t)).toBe('connection.retry.in{"seconds":3}');
    const past: RetryPlan = { kind: "scheduled", at: now - 1 };
    expect(retryLine(report(), past, now, t)).toBe('connection.retry.in{"seconds":0}');
  });

  it("says reconnecting with the attempt number", () => {
    const r = report({ outcome: "reconnecting", reconnectAttempt: 3 });
    expect(retryLine(r, { kind: "none" }, 0, t)).toBe('connection.retry.reconnecting{"attempt":3}');
  });

  it("says retries stopped only for auth", () => {
    expect(retryLine(report(), { kind: "stopped", reason: "auth" }, 0, t)).toBe("connection.retry.stoppedAuth");
    expect(retryLine(report(), { kind: "stopped", reason: "gave_up" }, 0, t)).toBe("connection.retry.onWake");
    expect(retryLine(report(), { kind: "stopped", reason: "disabled" }, 0, t)).toBe("connection.retry.disabled");
    expect(retryLine(report(), { kind: "none" }, 0, t)).toBeNull();
    expect(retryLine(report({ outcome: "registered" }), { kind: "scheduled", at: 5 }, 0, t)).toBeNull();
  });

  it("formats the registered line from expiry and last refresh", () => {
    const now = Date.UTC(2026, 10, 11, 15, 42, 18);
    const r = report({
      outcome: "registered",
      registered: { registeredAtMs: now - 20_000, expiresInS: 600, lastRefreshMs: now, publicAddr: null },
    });
    const line = registeredLine(r, now, t) ?? "";
    expect(line).toContain('"expires":"9 min 40 s"');
    expect(line).toMatch(/"at":"\d\d:\d\d:18"/);
    expect(registeredLine(report(), now, t)).toBeNull();
  });

  it("copies every step with status, detail and elapsed", () => {
    const text = copyDetails(report(), { kind: "stopped", reason: "gave_up" }, { appVersion: "0.6.7", os: "macOS", now: 0 }, t);
    expect(text).toContain("Aria 0.6.7 on macOS");
    expect(text).toContain("ghostbook@harness.lan · TLS · proxy 203.0.113.9:5061");
    expect(text).toContain("[skipped] connection.steps.resolve — literal address");
    expect(text).toContain("[ok] connection.steps.connect — handshake in 84 ms, certificate OK (84 ms)");
    expect(text).toContain("[failed] connection.steps.register — no reply in 32 s (sent 7 times) (32000 ms)");
    expect(text).toContain("[pending] connection.steps.authenticate");
    expect(text).toContain("Error: Registration timed out");
    expect(text).toContain("connection.retry.onWake");
    expect(text).toMatch(/Captured 1970-01-01T00:00:00/);
  });
});
