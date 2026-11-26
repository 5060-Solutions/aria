import { describe, expect, it } from "vitest";
import { appendSent, callLayout, digitsFor, KEYPAD_KEYS } from "./inCallKeypad";

describe("sent digits", () => {
  it("accumulates the digits of one call", () => {
    let sent = appendSent(null, "c1", "1");
    sent = appendSent(sent, "c1", "2");
    sent = appendSent(sent, "c1", "#");
    expect(digitsFor(sent, "c1")).toBe("12#");
  });

  it("starts again on another call", () => {
    const sent = appendSent(null, "c1", "5");
    expect(digitsFor(sent, "c2")).toBe("");
    expect(digitsFor(appendSent(sent, "c2", "7"), "c2")).toBe("7");
  });

  it("keeps only the tail of a long string", () => {
    let sent = null as ReturnType<typeof appendSent> | null;
    for (let i = 0; i < 40; i++) sent = appendSent(sent, "c1", String(i % 10));
    expect(digitsFor(sent, "c1")).toHaveLength(32);
  });
});

describe("keypad", () => {
  it("has the twelve keys", () => {
    expect(KEYPAD_KEYS.map((k) => k.digit).join("")).toBe("123456789*0#");
  });

  it("makes room for the keypad by dropping the avatar and the meter, not the caller", () => {
    expect(callLayout(true)).toEqual({ showAvatar: false, showMeter: false, showKeypad: true });
    expect(callLayout(false)).toEqual({ showAvatar: true, showMeter: true, showKeypad: false });
  });
});
