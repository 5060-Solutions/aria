import { describe, expect, it } from "vitest";
import { callerDisplay, contactLine, dialerMatches } from "./callerDisplay";
import type { Contact } from "../types/sip";

const contacts: Contact[] = [
  { id: "c1", name: "Pat Lee", uri: "sip:1002@test.ariaroute.com", favorite: false, source: "local" },
  { id: "c2", name: "Sam Rivera", uri: "sip:+12128402440@test.ariaroute.com", phone: "(212) 840-2440", favorite: true, source: "local" },
  { id: "c3", name: "Acme Support", uri: "sip:+14155550123@test.ariaroute.com", favorite: false, source: "google" },
];

describe("callerDisplay", () => {
  it("shows a formatted number, never the host, for an unknown caller", () => {
    expect(callerDisplay({ remoteUri: "sip:+12128402440@192.81.23.4" }, [], "US")).toEqual({
      primary: "+1 212 840 2440",
      secondary: null,
      withheld: false,
    });
    expect(callerDisplay({ remoteUri: "sip:2128402440@pbx.example" }, [], "US")).toEqual({
      primary: "+1 212 840 2440",
      secondary: null,
      withheld: false,
    });
  });

  it("names a matching contact, with the number underneath", () => {
    expect(callerDisplay({ remoteUri: "sip:+12128402440@192.81.23.4" }, contacts, "US")).toEqual({
      primary: "Sam Rivera",
      secondary: "+1 212 840 2440",
      withheld: false,
    });
    expect(callerDisplay({ remoteUri: "sip:1002@test.ariaroute.com" }, contacts, "US")).toEqual({
      primary: "Pat Lee",
      secondary: "1002",
      withheld: false,
    });
  });

  it("prefers the name the PBX sent, with the number underneath", () => {
    expect(callerDisplay({ remoteUri: "sip:+12128402440@192.81.23.4", remoteName: "Front Desk" }, [], "US")).toEqual({
      primary: "Front Desk",
      secondary: "+1 212 840 2440",
      withheld: false,
    });
  });

  it("does not repeat a 'name' that is just the number again", () => {
    expect(callerDisplay({ remoteUri: "sip:1002@pbx", remoteName: "1002" }, [], "US")).toEqual({
      primary: "1002",
      secondary: null,
      withheld: false,
    });
    expect(callerDisplay({ remoteUri: "sip:+12128402440@pbx", remoteName: "+12128402440" }, [], "US")).toEqual({
      primary: "+1 212 840 2440",
      secondary: null,
      withheld: false,
    });
  });

  it("keeps an extension as it is, and names a withheld caller as the UI asks", () => {
    expect(callerDisplay({ remoteUri: "sip:1002@pbx" }, [], "US")).toEqual({ primary: "1002", secondary: null, withheld: false });
    expect(callerDisplay({ remoteUri: "sip:anonymous@anonymous.invalid" }, [], "US")).toEqual({ primary: "anonymous", secondary: null, withheld: true });
    expect(callerDisplay({ remoteUri: "sip:anonymous@anonymous.invalid" }, [], "US", "Unknown caller")).toEqual({ primary: "Unknown caller", secondary: null, withheld: true });
    expect(callerDisplay({ remoteUri: "sip:Restricted@10.0.0.1", remoteName: "Restricted" }, [], "US", "Unknown caller").primary).toBe("Unknown caller");
  });
});

describe("dialerMatches", () => {
  it("matches contacts by name, case-insensitively, once letters are typed", () => {
    expect(dialerMatches("sa", contacts).map((c) => c.id)).toEqual(["c2"]);
    expect(dialerMatches("ACME", contacts).map((c) => c.id)).toEqual(["c3"]);
    expect(dialerMatches("lee", contacts).map((c) => c.id)).toEqual(["c1"]);
  });

  it("matches by digits too, so a partial number finds the contact", () => {
    expect(dialerMatches("212840", contacts).map((c) => c.id)).toEqual(["c2"]);
    expect(dialerMatches("1002", contacts).map((c) => c.id)).toEqual(["c1"]);
  });

  it("is empty for nothing typed or nothing matching, and favourites come first", () => {
    expect(dialerMatches("", contacts)).toEqual([]);
    expect(dialerMatches("zzz", contacts)).toEqual([]);
    const both: Contact[] = [contacts[2], { ...contacts[1], name: "Acme Sam" }];
    expect(dialerMatches("acme", both).map((c) => c.id)).toEqual(["c2", "c3"]);
  });
});

describe("contactLine", () => {
  it("is the formatted number, or the bare extension, without the phone's own domain", () => {
    expect(contactLine({ uri: "sip:+12128402440@test.ariaroute.com" }, "test.ariaroute.com", "US")).toBe("+1 212 840 2440");
    expect(contactLine({ uri: "sip:1002@test.ariaroute.com" }, "test.ariaroute.com", "US")).toBe("1002");
    expect(contactLine({ uri: "sip:1002" }, "test.ariaroute.com", "US")).toBe("1002");
  });

  it("keeps a foreign host, since that is what makes the address reachable", () => {
    expect(contactLine({ uri: "sip:1002@other-pbx.example" }, "test.ariaroute.com", "US")).toBe("1002@other-pbx.example");
  });

  it("adds a different phone number after a dot", () => {
    expect(contactLine({ uri: "sip:1002@test.ariaroute.com", phone: "(212) 840-2440" }, "test.ariaroute.com", "US")).toBe("1002 · (212) 840-2440");
    expect(contactLine({ uri: "sip:+12128402440@test.ariaroute.com", phone: "212-840-2440" }, "test.ariaroute.com", "US")).toBe("+1 212 840 2440");
  });
});
