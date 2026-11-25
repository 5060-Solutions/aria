import { describe, expect, it } from "vitest";
import { recordedCalls, showRecordingsNav, transcriptFor } from "./recordings";
import type { CallHistoryEntry, SipAccount } from "../types/sip";

function entry(over: Partial<CallHistoryEntry>): CallHistoryEntry {
  return { id: "c1", accountId: "a", remoteUri: "sip:1932@pbx", direction: "inbound", startTime: 1000, duration: 30, missed: false, ...over };
}
const account = (over: Partial<SipAccount>): SipAccount =>
  ({ id: "a", displayName: "A", username: "1001", domain: "pbx", password: "", transport: "udp", port: 5060, enabled: true, ...over }) as SipAccount;

describe("showRecordingsNav", () => {
  it("shows when a call was recorded, or an enabled account records automatically", () => {
    expect(showRecordingsNav([entry({ recordingPath: "/r/c1.wav" })], [account({})])).toBe(true);
    expect(showRecordingsNav([], [account({ autoRecord: true })])).toBe(true);
    expect(showRecordingsNav([], [account({ autoRecord: true, enabled: false })])).toBe(false);
    expect(showRecordingsNav([entry({})], [account({})])).toBe(false);
  });
});

describe("recordedCalls", () => {
  it("is the recorded calls only, newest first", () => {
    const older = entry({ id: "old", startTime: 1000, recordingPath: "/r/old.wav" });
    const newer = entry({ id: "new", startTime: 2000, recordingPath: "/r/new.wav" });
    expect(recordedCalls([older, entry({ id: "none" }), newer]).map((e) => e.id)).toEqual(["new", "old"]);
  });
});

describe("transcriptFor", () => {
  const insights = [
    { callId: "c1", createdAt: 1, durationSecs: 30, status: "complete" },
    { callId: "sip-xyz", createdAt: 2, durationSecs: 10, status: "complete" },
  ];
  it("matches a transcript by the call's id, or by its SIP Call-ID", () => {
    expect(transcriptFor(insights, entry({ id: "c1" }))?.callId).toBe("c1");
    expect(transcriptFor(insights, entry({ id: "c9", sipCallId: "sip-xyz" }))?.callId).toBe("sip-xyz");
    expect(transcriptFor(insights, entry({ id: "c9" }))).toBeNull();
  });
});
