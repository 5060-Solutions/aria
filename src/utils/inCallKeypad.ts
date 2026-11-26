/** The digits sent during one call, for the line above the in-call keypad. */
export interface SentDigits {
  callId: string;
  digits: string;
}

/** Longest string kept: the line shows the tail, the rest is of no use. */
const MAX_DIGITS = 32;

/** What was sent on this call; nothing when the record belongs to an earlier one. */
export function digitsFor(sent: SentDigits | null, callId: string): string {
  return sent && sent.callId === callId ? sent.digits : "";
}

export function appendSent(sent: SentDigits | null, callId: string, digit: string): SentDigits {
  const digits = (digitsFor(sent, callId) + digit).slice(-MAX_DIGITS);
  return { callId, digits };
}

export const KEYPAD_KEYS: ReadonlyArray<{ digit: string; letters: string }> = [
  { digit: "1", letters: "" },
  { digit: "2", letters: "ABC" },
  { digit: "3", letters: "DEF" },
  { digit: "4", letters: "GHI" },
  { digit: "5", letters: "JKL" },
  { digit: "6", letters: "MNO" },
  { digit: "7", letters: "PQRS" },
  { digit: "8", letters: "TUV" },
  { digit: "9", letters: "WXYZ" },
  { digit: "*", letters: "" },
  { digit: "0", letters: "+" },
  { digit: "#", letters: "" },
];

/**
 * How the call screen is laid out. With the keypad open it gives its room to
 * the keypad, so the avatar and the level meter go; the remote party (name,
 * number, timer) and what has been typed stay above it, and the controls
 * stay below it.
 */
export function callLayout(keypadOpen: boolean): { showAvatar: boolean; showMeter: boolean; showKeypad: boolean } {
  return { showAvatar: !keypadOpen, showMeter: !keypadOpen, showKeypad: keypadOpen };
}
