import { describe, expect, it } from "vitest";
import { dialUri, formatDialNumber, toSipUri } from "./sipUri";

describe("toSipUri", () => {
  it("uses a full sip:user@host as it is, lower-casing the scheme", () => {
    expect(toSipUri("sip:alice@example.com", "pbx")).toBe("sip:alice@example.com");
    expect(toSipUri("SIPS:alice@example.com", "pbx")).toBe("sips:alice@example.com");
  });

  it("treats a sip: prefix without a host like a bare number", () => {
    expect(toSipUri("sip:5551234", "pbx.example.com")).toBe("sip:5551234@pbx.example.com");
  });

  it("adds the scheme to anything with an @", () => {
    expect(toSipUri("bob@other.example", "pbx")).toBe("sip:bob@other.example");
  });

  it("strips formatting from a bare number and addresses it at the domain", () => {
    expect(toSipUri(" (555) 123-4567 ", "pbx.example.com")).toBe("sip:5551234567@pbx.example.com");
    expect(toSipUri("555.123.4567", "pbx")).toBe("sip:5551234567@pbx");
  });

  it("percent-encodes # so it does not end the user part", () => {
    expect(toSipUri("*123#", "pbx")).toBe("sip:*123%23@pbx");
  });
});

describe("formatDialNumber", () => {
  const us = "US";

  it("strips formatting when no rules are set", () => {
    expect(formatDialNumber("(303) 555-1234", null)).toBe("3035551234");
    expect(formatDialNumber("(303) 555-1234", {})).toBe("3035551234");
  });

  it("does not add a country code by default", () => {
    expect(formatDialNumber("(303) 555-1234", {}, us)).toBe("3035551234");
  });

  it("adds the country code when autoCountryCode is on", () => {
    expect(formatDialNumber("(303) 555-1234", { autoCountryCode: true }, us)).toBe("13035551234");
    expect(formatDialNumber("+44 20 7946 0958", { autoCountryCode: true }, us)).toBe("442079460958");
  });

  it("leaves an invalid number alone with autoCountryCode on", () => {
    expect(formatDialNumber("1234", { autoCountryCode: true }, us)).toBe("1234");
    expect(formatDialNumber("555-0100", { autoCountryCode: true }, us)).toBe("5550100");
  });

  it("removes stripPrefix with and without a leading +", () => {
    expect(formatDialNumber("13035551234", { stripPrefix: "1" })).toBe("3035551234");
    expect(formatDialNumber("+13035551234", { stripPrefix: "1" })).toBe("3035551234");
    expect(formatDialNumber("3035551234", { stripPrefix: "1" })).toBe("3035551234");
  });

  it("adds prependPrefix", () => {
    expect(formatDialNumber("3035551234", { prependPrefix: "9" })).toBe("93035551234");
  });

  it("strips before it prepends", () => {
    expect(formatDialNumber("013035551234", { stripPrefix: "01", prependPrefix: "9" })).toBe("93035551234");
    expect(formatDialNumber("+13035551234", { stripPrefix: "1", prependPrefix: "9" })).toBe("93035551234");
  });

  it("prepends after the country code was added", () => {
    expect(
      formatDialNumber("(303) 555-1234", { autoCountryCode: true, prependPrefix: "9" }, us),
    ).toBe("913035551234");
  });

  it("leaves feature codes untouched", () => {
    const rules = { autoCountryCode: true, stripPrefix: "9", prependPrefix: "9" };
    expect(formatDialNumber("*97", rules, us)).toBe("*97");
    expect(formatDialNumber("#31#", rules, us)).toBe("#31#");
    expect(formatDialNumber("*72 303 555 1234", rules, us)).toBe("*72 303 555 1234");
  });

  it("does not prepend to a number that starts with +", () => {
    expect(formatDialNumber("+13035551234", { prependPrefix: "9" })).toBe("+13035551234");
    expect(formatDialNumber("+44 20 7946 0958", { prependPrefix: "9" })).toBe("+442079460958");
  });

  it("leaves anything with an @ or a full sip: URI untouched", () => {
    const rules = { autoCountryCode: true, stripPrefix: "1", prependPrefix: "9" };
    expect(formatDialNumber("alice@example.com", rules, us)).toBe("alice@example.com");
    expect(formatDialNumber("sip:13035551234@pbx", rules, us)).toBe("sip:13035551234@pbx");
    expect(formatDialNumber("sips:alice@example.com", rules, us)).toBe("sips:alice@example.com");
  });

  it("applies the rules to the user part of a sip: contact with no host", () => {
    expect(formatDialNumber("sip:13035551234", { stripPrefix: "1", prependPrefix: "9" })).toBe("93035551234");
    expect(formatDialNumber("sip:*97", { prependPrefix: "9" })).toBe("*97");
  });

  it("trims whitespace", () => {
    expect(formatDialNumber("  3035551234  ", {})).toBe("3035551234");
    expect(formatDialNumber("  alice@example.com ", {})).toBe("alice@example.com");
    expect(formatDialNumber("   ", { prependPrefix: "9" })).toBe("");
  });
});

describe("dialUri", () => {
  const account = { domain: "pbx.example.com", stripPrefix: "1", prependPrefix: "9" };

  it("applies the rules and addresses the result at the account domain", () => {
    expect(dialUri("(303) 555-1234", account, "US")).toBe("sip:93035551234@pbx.example.com");
    expect(dialUri("sip:13035551234", account, "US")).toBe("sip:93035551234@pbx.example.com");
  });

  it("keeps feature codes and full URIs intact", () => {
    expect(dialUri("*97", account)).toBe("sip:*97@pbx.example.com");
    expect(dialUri("sip:bob@other.example", account)).toBe("sip:bob@other.example");
  });

  it("falls back to an empty domain without an account", () => {
    expect(dialUri("5551234", undefined)).toBe("sip:5551234@");
  });
});
