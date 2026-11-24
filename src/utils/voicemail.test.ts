import { describe, expect, it } from "vitest";
import {
  callerLabel,
  formatVoicemailDuration,
  mailboxFor,
  sortNewestFirst,
  unreadCount,
  type VoicemailMessage,
} from "./voicemail";
import type { Contact, SipAccount } from "../types/sip";

function account(overrides: Partial<SipAccount> = {}): SipAccount {
  return {
    id: "acc-1",
    displayName: "Front Desk",
    username: "1001",
    domain: "regular-test.ariaroute.com",
    password: "",
    transport: "tls",
    port: 5061,
    enabled: true,
    ...overrides,
  };
}

function message(overrides: Partial<VoicemailMessage> = {}): VoicemailMessage {
  return {
    id: "m1",
    callerId: "+15551230002",
    durationSecs: 42,
    read: false,
    createdAt: "2026-11-24T10:00:00Z",
    transcription: null,
    ...overrides,
  };
}

describe("mailboxFor", () => {
  it("names the mailbox of an account that has a PBX API", () => {
    expect(mailboxFor(account({ pbxApiUrl: "https://api.ariaroute.com" }))).toEqual({
      accountId: "acc-1",
      apiUrl: "https://api.ariaroute.com",
      extension: "1001",
      domain: "regular-test.ariaroute.com",
    });
  });

  it("is null without an API, so the button dials instead", () => {
    expect(mailboxFor(account())).toBeNull();
    expect(mailboxFor(account({ pbxApiUrl: "   " }))).toBeNull();
    expect(mailboxFor(undefined)).toBeNull();
  });
});

describe("messages", () => {
  it("sorts newest first whatever order the server used", () => {
    const older = message({ id: "old", createdAt: "2026-11-23T09:00:00Z" });
    const newer = message({ id: "new", createdAt: "2026-11-24T10:00:00Z" });
    expect(sortNewestFirst([older, newer]).map((m) => m.id)).toEqual(["new", "old"]);
    expect(sortNewestFirst([newer, older]).map((m) => m.id)).toEqual(["new", "old"]);
  });

  it("counts the unread ones", () => {
    expect(unreadCount([message({ read: false }), message({ id: "m2", read: true }), message({ id: "m3" })])).toBe(2);
    expect(unreadCount([])).toBe(0);
  });

  it("formats a duration as minutes and seconds", () => {
    expect(formatVoicemailDuration(0)).toBe("0:00");
    expect(formatVoicemailDuration(5)).toBe("0:05");
    expect(formatVoicemailDuration(42)).toBe("0:42");
    expect(formatVoicemailDuration(65)).toBe("1:05");
    expect(formatVoicemailDuration(3600)).toBe("60:00");
  });
});

describe("callerLabel", () => {
  const contacts: Contact[] = [
    { id: "c1", name: "Pat Lee", uri: "sip:1002@regular-test.ariaroute.com", favorite: false, source: "local" },
    { id: "c2", name: "Sam Rivera", uri: "sip:+15551230002@regular-test.ariaroute.com", phone: "(555) 123-0002", favorite: false, source: "local" },
  ];

  it("names a contact whose extension or number matches", () => {
    expect(callerLabel("1002", contacts)).toBe("Pat Lee");
    expect(callerLabel("+15551230002", contacts)).toBe("Sam Rivera");
    expect(callerLabel("15551230002", contacts)).toBe("Sam Rivera");
  });

  it("falls back to the caller id, and to null when there is none", () => {
    expect(callerLabel("+15559990000", contacts)).toBe("+15559990000");
    expect(callerLabel("", contacts)).toBeNull();
    expect(callerLabel("unknown", contacts)).toBeNull();
    expect(callerLabel("anonymous", [])).toBeNull();
  });
});
