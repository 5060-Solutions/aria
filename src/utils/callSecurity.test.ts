import { describe, expect, it } from "vitest";
import { mediaSecurity, sdesKeysExposed } from "./callSecurity";

describe("mediaSecurity", () => {
  it("is encrypted only with SRTP over TLS signalling", () => {
    expect(mediaSecurity(true, true)).toBe("encrypted");
  });

  it("does not call SDES over UDP/TCP encrypted", () => {
    expect(mediaSecurity(true, false)).toBe("keysInClear");
    expect(mediaSecurity(true, undefined)).toBe("keysInClear");
  });

  it("is none without SRTP, whatever the transport", () => {
    expect(mediaSecurity(false, true)).toBe("none");
    expect(mediaSecurity(false, false)).toBe("none");
  });
});

describe("sdesKeysExposed", () => {
  it("warns for SDES on anything but TLS", () => {
    expect(sdesKeysExposed("sdes", "udp")).toBe(true);
    expect(sdesKeysExposed("sdes", "tcp")).toBe(true);
    expect(sdesKeysExposed("sdes", "auto")).toBe(true);
    expect(sdesKeysExposed("sdes", "tls")).toBe(false);
  });

  it("does not warn when SRTP is off", () => {
    expect(sdesKeysExposed("disabled", "udp")).toBe(false);
    expect(sdesKeysExposed(undefined, "udp")).toBe(false);
  });
});
