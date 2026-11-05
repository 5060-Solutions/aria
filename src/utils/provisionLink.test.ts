import { describe, expect, it } from "vitest";
import { isProvisionLink, parseProvisionLink } from "./provisionLink";
import { deepLinkTarget } from "./deepLink";

const REDEEM = "https://api.ariaroute.com/provision/softphone/tok_ABC123";
const link = (url: string, v = "1") => `aria://provision?v=${v}&url=${encodeURIComponent(url)}`;

describe("parseProvisionLink", () => {
  it("accepts the contract's link", () => {
    expect(parseProvisionLink(link(REDEEM))).toEqual({
      ok: true,
      link: { redeemUrl: REDEEM, serverHost: "api.ariaroute.com" },
    });
  });

  it("accepts a trailing slash, surrounding whitespace and reordered params", () => {
    const reordered = `aria://provision/?url=${encodeURIComponent(REDEEM)}&v=1`;
    expect(parseProvisionLink(reordered).ok).toBe(true);
    expect(parseProvisionLink(`  ${link(REDEEM)}\n`).ok).toBe(true);
  });

  it("keeps a non-default port in the host shown to the user", () => {
    const r = parseProvisionLink(link("https://pbx.example.com:8443/provision/softphone/t"));
    expect(r.ok && r.link.serverHost).toBe("pbx.example.com:8443");
  });

  it("rejects plain http to anywhere but a local stub", () => {
    expect(parseProvisionLink(link("http://api.ariaroute.com/provision/softphone/t"))).toEqual({
      ok: false,
      reason: "url is not https",
    });
    expect(
      parseProvisionLink(link("http://api.ariaroute.com/provision/softphone/t"), { allowLocalHttp: true }).ok,
    ).toBe(false);
  });

  it("allows http to localhost only when asked", () => {
    const local = link("http://localhost:8787/provision/softphone/t");
    const loopback = link("http://127.0.0.1:8787/provision/softphone/t");
    expect(parseProvisionLink(local).ok).toBe(false);
    expect(parseProvisionLink(loopback).ok).toBe(false);
    expect(parseProvisionLink(local, { allowLocalHttp: true }).ok).toBe(true);
    expect(parseProvisionLink(loopback, { allowLocalHttp: true }).ok).toBe(true);
  });

  it("rejects userinfo", () => {
    for (const u of [
      "https://user:pw@api.ariaroute.com/provision/softphone/t",
      "https://user@api.ariaroute.com/provision/softphone/t",
    ]) {
      expect(parseProvisionLink(link(u))).toEqual({ ok: false, reason: "url has userinfo" });
    }
  });

  it("rejects the wrong path", () => {
    for (const u of [
      "https://api.ariaroute.com/",
      "https://api.ariaroute.com/provision/softphone/",
      "https://api.ariaroute.com/provision/softphone",
      "https://api.ariaroute.com/provision/other/t",
      "https://api.ariaroute.com/evil/provision/softphone/t",
    ]) {
      expect(parseProvisionLink(link(u))).toEqual({ ok: false, reason: "url has the wrong path" });
    }
  });

  it("rejects a missing or garbled url", () => {
    expect(parseProvisionLink("aria://provision?v=1").ok).toBe(false);
    expect(parseProvisionLink("aria://provision?v=1&url=").ok).toBe(false);
    expect(parseProvisionLink("aria://provision?v=1&url=not%20a%20url").ok).toBe(false);
    expect(parseProvisionLink(`${link(REDEEM)}&url=${encodeURIComponent(REDEEM)}`).ok).toBe(false);
    expect(parseProvisionLink(link("javascript:alert(1)")).ok).toBe(false);
    expect(parseProvisionLink(link("file:///provision/softphone/t")).ok).toBe(false);
  });

  it("rejects any version but 1", () => {
    for (const v of ["2", "0", "", "1.0", "01"]) {
      expect(parseProvisionLink(link(REDEEM, v))).toEqual({ ok: false, reason: "unsupported version" });
    }
    expect(parseProvisionLink(`aria://provision?url=${encodeURIComponent(REDEEM)}`).ok).toBe(false);
    expect(parseProvisionLink(`${link(REDEEM)}&v=1`).ok).toBe(false);
  });

  it("rejects other schemes and other aria links", () => {
    for (const l of [
      `https://provision?v=1&url=${encodeURIComponent(REDEEM)}`,
      `tel:+15551234567`,
      `sip:1001@example.com`,
      `aria://dial?v=1&url=${encodeURIComponent(REDEEM)}`,
      `aria://provisioning?v=1&url=${encodeURIComponent(REDEEM)}`,
      `xaria://provision?v=1&url=${encodeURIComponent(REDEEM)}`,
      "",
    ]) {
      expect(parseProvisionLink(l).ok).toBe(false);
    }
  });

  it("does not throw on bad percent sequences", () => {
    for (const l of [
      "aria://provision?v=1&url=%",
      "aria://provision?v=1&url=%E0%A4%A",
      "aria://provision?v=%ZZ&url=https%3A%2F%2Fapi.ariaroute.com%2Fprovision%2Fsoftphone%2F%",
      "aria://provision?%%%",
    ]) {
      expect(() => parseProvisionLink(l)).not.toThrow();
      expect(parseProvisionLink(l).ok).toBe(false);
    }
  });

  it("never puts the token in a refusal reason", () => {
    const r = parseProvisionLink(link("http://api.ariaroute.com/provision/softphone/tok_ABC123"));
    expect(r.ok).toBe(false);
    expect(JSON.stringify(r)).not.toContain("tok_ABC123");
  });
});

describe("isProvisionLink", () => {
  it("recognises provision links of any version, and nothing else", () => {
    expect(isProvisionLink(link(REDEEM))).toBe(true);
    expect(isProvisionLink("aria://provision?server=pbx&user=1&pass=eA==")).toBe(true);
    expect(isProvisionLink("ARIA://provision?v=1")).toBe(true);
    expect(isProvisionLink("aria://provisioning?v=1")).toBe(false);
    expect(isProvisionLink("tel:123")).toBe(false);
  });

  it("is never mistaken for a dial target", () => {
    expect(deepLinkTarget(link(REDEEM))).toBeNull();
  });
});
