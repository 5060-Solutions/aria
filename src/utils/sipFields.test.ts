import { describe, expect, it } from "vitest";
import { isHost, isSipUser, normalizeProxy } from "./sipFields";

describe("sip field checks", () => {
  it("accepts hosts and refuses anything that could break a header", () => {
    expect(isHost("sip.ariaroute.com")).toBe(true);
    expect(isHost("192.0.2.10")).toBe(true);
    for (const bad of ["", "a b", "evil.com\r\nVia: x", "host:5061", "-x.com", ".x.com", "x>y"]) {
      expect(isHost(bad)).toBe(false);
    }
  });

  it("accepts SIP users", () => {
    expect(isSipUser("1001")).toBe(true);
    expect(isSipUser("alice.smith")).toBe(true);
    for (const bad of ["", "10 01", "1001@evil", "a:b", "a<b", "a\nb"]) {
      expect(isSipUser(bad)).toBe(false);
    }
  });

  it("normalises proxies", () => {
    expect(normalizeProxy("sip.ariaroute.com")).toBe("sip.ariaroute.com");
    expect(normalizeProxy("SIPS:Sip.AriaRoute.com:5061")).toBe("sip.ariaroute.com:5061");
    expect(normalizeProxy("sip:proxy.example.com:5070")).toBe("proxy.example.com:5070");
    for (const bad of ["host;transport=tls", "host:0", "host:99999", "host:x", "a b", "h:1:2", ""]) {
      expect(normalizeProxy(bad)).toBeNull();
    }
  });
});
