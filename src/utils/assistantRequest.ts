/**
 * What the backend sends (`mcp-call-request`) when an assistant asks to act
 * on calls, and what the consent dialog says about each kind.
 */

/** `place` dials a new call; the others act on an existing call. */
export type AssistantRequestKind = "place" | "answer" | "transfer" | "dtmf";

export interface AssistantRequest {
  requestId: string;
  kind: AssistantRequestKind;
  /**
   * What the dialog shows in its box: the number to call, the caller to
   * answer, the transfer target, or the first digits to send.
   */
  number: string;
  accountId: string | null;
  /** The existing call acted on; null for `place`. */
  callId: string | null;
  /** The other party of that call; null for `place`. */
  remote: string | null;
}

/** The `mcp.*` title and body keys for each kind of request. */
export const REQUEST_TEXT: Record<AssistantRequestKind, { title: string; body: string }> = {
  place: { title: "mcp.callRequestTitle", body: "mcp.callRequestBody" },
  answer: { title: "mcp.answerRequestTitle", body: "mcp.answerRequestBody" },
  transfer: { title: "mcp.transferRequestTitle", body: "mcp.transferRequestBody" },
  dtmf: { title: "mcp.dtmfRequestTitle", body: "mcp.dtmfRequestBody" },
};

/** The text keys for `kind`; anything unrecognised is shown as a new call. */
export function requestText(kind: string): { title: string; body: string } {
  return REQUEST_TEXT[kind as AssistantRequestKind] ?? REQUEST_TEXT.place;
}
