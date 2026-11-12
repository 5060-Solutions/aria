import { describe, expect, it } from "vitest";
import { shouldPlayRingback } from "./ringback";

describe("shouldPlayRingback", () => {
  it("returns false when there is no active call", () => {
    expect(shouldPlayRingback(null)).toBe(false);
    expect(shouldPlayRingback(undefined)).toBe(false);
  });

  it("plays local ringback for dialing or ringing calls without early media", () => {
    expect(shouldPlayRingback({ state: "dialing", hasEarlyMedia: false })).toBe(true);
    expect(shouldPlayRingback({ state: "ringing", hasEarlyMedia: false })).toBe(true);
    expect(shouldPlayRingback({ state: "ringing" })).toBe(true);
  });

  it("suppresses local ringback when early media is present (RFC 3960)", () => {
    expect(shouldPlayRingback({ state: "dialing", hasEarlyMedia: true })).toBe(false);
    expect(shouldPlayRingback({ state: "ringing", hasEarlyMedia: true })).toBe(false);
  });

  it("returns false for non-ringing states", () => {
    expect(shouldPlayRingback({ state: "incoming", hasEarlyMedia: false })).toBe(false);
    expect(shouldPlayRingback({ state: "connected", hasEarlyMedia: false })).toBe(false);
    expect(shouldPlayRingback({ state: "held", hasEarlyMedia: false })).toBe(false);
    expect(shouldPlayRingback({ state: "ended", hasEarlyMedia: false })).toBe(false);
  });
});
