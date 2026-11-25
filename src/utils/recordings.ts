import type { CallHistoryEntry, SipAccount } from "../types/sip";

/** The summary row `ai_insights` returns for a transcript. */
export interface TranscriptSummary {
  callId: string;
  createdAt: number;
  durationSecs: number;
  status: string;
}

/**
 * Whether Recordings belongs in the left bar: once a call has been recorded,
 * or when an enabled account records every call, so the place where the
 * recordings land is visible before the first one.
 */
export function showRecordingsNav(
  history: ReadonlyArray<CallHistoryEntry>,
  accounts: ReadonlyArray<Pick<SipAccount, "enabled" | "autoRecord">>,
): boolean {
  return history.some((e) => !!e.recordingPath) || accounts.some((a) => a.enabled && !!a.autoRecord);
}

/** The calls that have a recording, newest first. */
export function recordedCalls(history: ReadonlyArray<CallHistoryEntry>): CallHistoryEntry[] {
  return history.filter((e) => !!e.recordingPath).sort((a, b) => b.startTime - a.startTime);
}

/**
 * The transcript stored for a call. Transcripts are keyed by the call id the
 * backend recorded under, which is the history entry's id; an older build
 * keyed some by the SIP Call-ID, so that is tried too.
 */
export function transcriptFor(
  insights: ReadonlyArray<TranscriptSummary>,
  entry: Pick<CallHistoryEntry, "id" | "sipCallId">,
): TranscriptSummary | null {
  return insights.find((i) => i.callId === entry.id) ?? (entry.sipCallId ? insights.find((i) => i.callId === entry.sipCallId) : undefined) ?? null;
}
