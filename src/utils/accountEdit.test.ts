import { describe, expect, it } from "vitest";
import { keptProvisioning } from "./accountEdit";
import type { SipAccount } from "../types/sip";

const provisioned: SipAccount = {
  id: "a",
  displayName: "Front Desk",
  username: "1001",
  domain: "regular-test.ariaroute.com",
  password: "",
  transport: "tls",
  port: 5061,
  outboundProxy: "sip.ariaroute.com:5061",
  tlsServerName: "sip.ariaroute.com",
  provisionedFrom: "api.ariaroute.com",
  voicemailNumber: "8500",
  enabled: true,
};
const same = { username: "1001", domain: "Regular-Test.AriaRoute.com", outboundProxy: "sip.ariaroute.com:5061" };

describe("keptProvisioning", () => {
  it("keeps all three while the line and proxy are unchanged", () => {
    expect(keptProvisioning(provisioned, same)).toEqual({
      tlsServerName: "sip.ariaroute.com",
      provisionedFrom: "api.ariaroute.com",
      voicemailNumber: "8500",
    });
  });

  it("drops the link fields when the username changes, keeping the PBX's voicemail number", () => {
    expect(keptProvisioning(provisioned, { ...same, username: "2002" })).toEqual({
      tlsServerName: undefined,
      provisionedFrom: undefined,
      voicemailNumber: "8500",
    });
  });

  it("drops everything when the domain changes", () => {
    expect(keptProvisioning(provisioned, { ...same, domain: "other.example.com" })).toEqual({
      tlsServerName: undefined,
      provisionedFrom: undefined,
      voicemailNumber: undefined,
    });
  });

  it("drops only the TLS name when the proxy changes", () => {
    expect(keptProvisioning(provisioned, { ...same, outboundProxy: "proxy.example.com" })).toEqual({
      tlsServerName: undefined,
      provisionedFrom: "api.ariaroute.com",
      voicemailNumber: "8500",
    });
  });

  it("sets none for a new account", () => {
    expect(keptProvisioning(null, same)).toEqual({
      tlsServerName: undefined,
      provisionedFrom: undefined,
      voicemailNumber: undefined,
    });
  });
});
