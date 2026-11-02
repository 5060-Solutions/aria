import { describe, expect, it } from "vitest";
import { sanitizeMexicanPhoneNumber } from "./phone";

describe("sanitizeMexicanPhoneNumber", () => {
  it("returns empty string for empty, whitespace, or falsy inputs", () => {
    expect(sanitizeMexicanPhoneNumber("")).toBe("");
    expect(sanitizeMexicanPhoneNumber("   ")).toBe("");
    expect(sanitizeMexicanPhoneNumber(null as unknown as string)).toBe("");
    expect(sanitizeMexicanPhoneNumber(undefined as unknown as string)).toBe("");
  });

  it("leaves SIP URIs and user@host addresses untouched", () => {
    expect(sanitizeMexicanPhoneNumber("sip:alice@pbx.example.com")).toBe("sip:alice@pbx.example.com");
    expect(sanitizeMexicanPhoneNumber("bob@example.com")).toBe("bob@example.com");
    expect(sanitizeMexicanPhoneNumber("1234@10.0.0.1")).toBe("1234@10.0.0.1");
  });

  it("strips common formatting (spaces, dashes, parentheses, dots, slashes)", () => {
    expect(sanitizeMexicanPhoneNumber("(81) 1234-5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("81.1234.5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("81-1234-5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("81/1234/5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("  81 1234 5678  ")).toBe("8112345678");
  });

  it("normalizes Mexican numbers with +521 to 10 digits", () => {
    expect(sanitizeMexicanPhoneNumber("+5218112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("+52 1 81 1234 5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("+52 (1) 81-1234-5678")).toBe("8112345678");
  });

  it("normalizes Mexican numbers with +52 to 10 digits", () => {
    expect(sanitizeMexicanPhoneNumber("+528112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("+52 81 1234 5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("+52 (81) 1234-5678")).toBe("8112345678");
  });

  it("normalizes Mexican numbers with 0052 to 10 digits", () => {
    expect(sanitizeMexicanPhoneNumber("00528112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("0052 81 1234 5678")).toBe("8112345678");
  });

  it("normalizes Mexican numbers with 521 or 52 prefix to 10 digits", () => {
    expect(sanitizeMexicanPhoneNumber("5218112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("528112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("52 81 1234 5678")).toBe("8112345678");
  });

  it("normalizes legacy domestic Mexican mobile prefixes (044, 045)", () => {
    expect(sanitizeMexicanPhoneNumber("0448112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("044 81 1234 5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("0458112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("045 81 1234 5678")).toBe("8112345678");
  });

  it("normalizes legacy domestic Mexican long-distance prefix (01)", () => {
    expect(sanitizeMexicanPhoneNumber("018112345678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("01 81 1234 5678")).toBe("8112345678");
    expect(sanitizeMexicanPhoneNumber("01 (81) 1234-5678")).toBe("8112345678");
  });

  it("preserves short PBX extensions", () => {
    expect(sanitizeMexicanPhoneNumber("101")).toBe("101");
    expect(sanitizeMexicanPhoneNumber("9998")).toBe("9998");
    expect(sanitizeMexicanPhoneNumber("*97")).toBe("*97");
    expect(sanitizeMexicanPhoneNumber("#123")).toBe("#123");
  });

  it("preserves international numbers from other countries", () => {
    expect(sanitizeMexicanPhoneNumber("+1 303 555 1234")).toBe("+13035551234");
    expect(sanitizeMexicanPhoneNumber("+34 91 123 4567")).toBe("+34911234567");
    expect(sanitizeMexicanPhoneNumber("+44 20 7946 0958")).toBe("+442079460958");
  });
});
