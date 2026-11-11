import { describe, expect, it } from "vitest";
import { hintKey } from "./connectionHints";
import type { ConnectionReport, ConnectionStep, FailureKind } from "../types/connection";
import en from "../locales/en.json";
import es from "../locales/es.json";
import fr from "../locales/fr.json";
import de from "../locales/de.json";

function step(kind: ConnectionStep["kind"], status: ConnectionStep["status"], failure: FailureKind | null = null): ConnectionStep {
  return { kind, status, detail: null, elapsedMs: null, failure };
}

function report(steps: ConnectionStep[]): ConnectionReport {
  return {
    accountId: "a1",
    username: "1001",
    domain: "pbx.example.com",
    transport: "tls",
    target: { host: "pbx.example.com", port: 5061, viaProxy: false },
    steps,
    outcome: "error",
    error: null,
    reconnectAttempt: 0,
    registered: null,
    attemptStartedMs: 0,
    attemptFinishedMs: null,
  };
}

describe("hintKey", () => {
  it("maps the failed step's kind to a hint key", () => {
    const r = report([step("resolve", "ok"), step("connect", "failed", "tls_untrusted"), step("register", "pending")]);
    expect(hintKey(r)).toBe("connection.hints.tls_untrusted");
  });

  it("has no hint for an unclassified failure", () => {
    expect(hintKey(report([step("register", "failed", "other")]))).toBeNull();
    expect(hintKey(report([step("register", "failed", null)]))).toBeNull();
  });

  it("has no hint when nothing failed", () => {
    expect(hintKey(report([step("resolve", "ok"), step("registered", "ok")]))).toBeNull();
  });

  it("every locale has every connection string", () => {
    const flatten = (o: unknown, prefix = ""): string[] =>
      typeof o === "object" && o !== null
        ? Object.entries(o).flatMap(([k, v]) => flatten(v, prefix ? `${prefix}.${k}` : k))
        : [prefix];
    const keys = flatten((en as { connection: unknown }).connection, "connection");
    expect(keys.length).toBeGreaterThan(20);
    for (const [name, locale] of [["es", es], ["fr", fr], ["de", de]] as const) {
      const have = new Set(flatten((locale as { connection?: unknown }).connection ?? {}, "connection"));
      const missing = keys.filter((k) => !have.has(k));
      expect(missing, `${name} is missing`).toEqual([]);
    }
  });
});
