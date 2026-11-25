import { describe, expect, it } from "vitest";
import { filterLogs } from "./diagnosticsFilter";
import type { DiagnosticLog } from "../components/diagnostics/types";

function entry(over: Partial<DiagnosticLog>): DiagnosticLog {
  return {
    timestamp: 1,
    accountId: "a",
    direction: "sent",
    remoteAddr: "10.0.0.1:5060",
    summary: "REGISTER sip:pbx",
    raw: "REGISTER sip:pbx SIP/2.0\r\nCSeq: 1 REGISTER\r\nCall-ID: reg-1\r\n\r\n",
    callId: "reg-1",
    ...over,
  };
}

const invite = entry({ summary: "INVITE sip:1002@pbx", raw: "INVITE sip:1002@pbx SIP/2.0\r\nCSeq: 1 INVITE\r\nCall-ID: call-1\r\n\r\n", callId: "call-1" });
const options = entry({ summary: "OPTIONS sip:pbx", raw: "OPTIONS sip:pbx SIP/2.0\r\nCSeq: 9 OPTIONS\r\n\r\n", callId: undefined, accountId: "b" });
const app = entry({ direction: "app", remoteAddr: "aria", summary: "Assistant placed a call", raw: "to 1002", callId: undefined });
const all = [entry({}), invite, options, app];
const everything = new Set(["calls", "register", "keepalive", "other"] as const);

describe("filterLogs", () => {
  it("keeps everything with no narrowing", () => {
    expect(filterLogs(all, { categories: everything, search: "", callId: null })).toHaveLength(4);
  });

  it("narrows to one account", () => {
    expect(filterLogs(all, { accountId: "b", categories: everything, search: "", callId: null })).toEqual([options]);
  });

  it("files app events under Other and drops categories that are off", () => {
    const calls = filterLogs(all, { categories: new Set(["calls"]), search: "", callId: null });
    expect(calls).toEqual([invite]);
    const other = filterLogs(all, { categories: new Set(["other"]), search: "", callId: null });
    expect(other).toEqual([app]);
  });

  it("searches summary, raw text, address and Call-ID, ignoring case", () => {
    expect(filterLogs(all, { categories: everything, search: "invite", callId: null })).toEqual([invite]);
    expect(filterLogs(all, { categories: everything, search: "CALL-1", callId: null })).toEqual([invite]);
    expect(filterLogs(all, { categories: everything, search: "assistant", callId: null })).toEqual([app]);
    expect(filterLogs(all, { categories: everything, search: "  ", callId: null })).toHaveLength(4);
  });

  it("shows one call when a Call-ID is picked", () => {
    expect(filterLogs(all, { categories: everything, search: "", callId: "call-1" })).toEqual([invite]);
  });
});
