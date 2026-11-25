import { describe, expect, it } from "vitest";
import {
  conversationName,
  mergeThread,
  normalizeRecipient,
  smsSegments,
  sortByLastAt,
  unseenCount,
  type Conversation,
  type SmsMessage,
} from "./messaging";
import type { Contact } from "../types/sip";

function contact(overrides: Partial<Contact> = {}): Contact {
  return { id: "c1", name: "Ada Lovelace", uri: "sip:+12125550123@test.ariaroute.com", favorite: false, source: "local", ...overrides };
}

function conversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    remoteNumber: "+12125550123",
    messageCount: 1,
    lastBody: "hi",
    lastDirection: "inbound",
    lastStatus: "received",
    lastAt: "2026-11-24T10:00:00Z",
    lastHasMedia: false,
    optedOut: false,
    ...overrides,
  };
}

function message(overrides: Partial<SmsMessage> = {}): SmsMessage {
  return {
    id: "s1",
    direction: "inbound",
    remoteNumber: "+12125550123",
    body: "hi",
    status: "received",
    autoReply: false,
    errorMessage: null,
    createdAt: "2026-11-24T10:00:00Z",
    attachments: [],
    ...overrides,
  };
}

describe("smsSegments", () => {
  it("counts a short GSM-7 text as one segment of 160", () => {
    expect(smsSegments("See you at 4")).toEqual({ segments: 1, chars: 12, perSegment: 160, encoding: "gsm7" });
  });

  it("splits a long GSM-7 text into 153-character parts", () => {
    expect(smsSegments("a".repeat(160)).segments).toBe(1);
    expect(smsSegments("a".repeat(161))).toMatchObject({ segments: 2, perSegment: 153 });
    expect(smsSegments("a".repeat(306)).segments).toBe(2);
    expect(smsSegments("a".repeat(307)).segments).toBe(3);
  });

  it("counts GSM extension characters twice", () => {
    // 159 letters plus a euro sign is 161 septets: two segments.
    expect(smsSegments(`${"a".repeat(159)}€`).segments).toBe(2);
    expect(smsSegments("[]{}^~\\|€").chars).toBe(18);
  });

  it("uses UCS-2 limits once any character is outside GSM-7", () => {
    expect(smsSegments("Dziękuję")).toEqual({ segments: 1, chars: 8, perSegment: 70, encoding: "ucs2" });
    expect(smsSegments("é".repeat(70)).segments).toBe(1);
    expect(smsSegments("ę".repeat(71))).toMatchObject({ segments: 2, perSegment: 67 });
    // An emoji is a surrogate pair: two UTF-16 units.
    expect(smsSegments("👍").chars).toBe(2);
  });

  it("is zero segments for nothing to send", () => {
    expect(smsSegments("").segments).toBe(0);
    expect(smsSegments("   ").segments).toBe(0);
  });
});

describe("normalizeRecipient", () => {
  it("turns a national number into E.164 for the default country", () => {
    expect(normalizeRecipient("(212) 555-0123", "US")).toBe("+12125550123");
    expect(normalizeRecipient("212 555 0123", "US")).toBe("+12125550123");
  });

  it("keeps an international number as typed", () => {
    expect(normalizeRecipient("+44 20 7946 0958", "US")).toBe("+442079460958");
  });

  it("refuses what is not a dialable phone number", () => {
    expect(normalizeRecipient("Ada", "US")).toBeNull();
    expect(normalizeRecipient("1001", "US")).toBeNull();
    expect(normalizeRecipient("", "US")).toBeNull();
  });
});

describe("conversationName", () => {
  it("names a contact and shows the number under it", () => {
    expect(conversationName("+12125550123", [contact()], "US")).toEqual({
      primary: "Ada Lovelace",
      secondary: "+1 212 555 0123",
    });
  });

  it("matches a contact by phone field and by national digits", () => {
    const c = contact({ uri: "sip:1001@test.ariaroute.com", phone: "2125550123" });
    expect(conversationName("+12125550123", [c], "US").primary).toBe("Ada Lovelace");
  });

  it("falls back to the formatted number alone", () => {
    expect(conversationName("+12125550123", [], "US")).toEqual({ primary: "+1 212 555 0123", secondary: null });
  });

  it("leaves a short code as it is", () => {
    expect(conversationName("87365", [], "US")).toEqual({ primary: "87365", secondary: null });
  });
});

describe("sortByLastAt", () => {
  it("puts the newest conversation first", () => {
    const old = conversation({ remoteNumber: "+1", lastAt: "2026-11-23T10:00:00Z" });
    const recent = conversation({ remoteNumber: "+2", lastAt: "2026-11-24T10:00:00Z" });
    expect(sortByLastAt([old, recent]).map((c) => c.remoteNumber)).toEqual(["+2", "+1"]);
  });
});

describe("unseenCount", () => {
  it("counts conversations whose last inbound text is newer than when they were last opened", () => {
    const conversations = [
      conversation({ remoteNumber: "+1", lastAt: "2026-11-24T10:00:00Z" }),
      conversation({ remoteNumber: "+2", lastAt: "2026-11-24T09:00:00Z" }),
      conversation({ remoteNumber: "+3", lastAt: "2026-11-24T11:00:00Z", lastDirection: "outbound" }),
    ];
    const seen = { "+1": "2026-11-24T09:30:00Z", "+2": "2026-11-24T09:00:00Z" };
    expect(unseenCount(conversations, seen)).toBe(1);
    expect(unseenCount(conversations, {})).toBe(2);
  });
});

describe("mergeThread", () => {
  it("adds older pages and new sends without duplicates, newest first", () => {
    const have = [message({ id: "s3", createdAt: "2026-11-24T12:00:00Z" }), message({ id: "s2", createdAt: "2026-11-24T11:00:00Z" })];
    const older = [message({ id: "s2", createdAt: "2026-11-24T11:00:00Z" }), message({ id: "s1", createdAt: "2026-11-24T10:00:00Z" })];
    const sent = message({ id: "s4", direction: "outbound", createdAt: "2026-11-24T13:00:00Z" });
    expect(mergeThread(have, older).map((m) => m.id)).toEqual(["s3", "s2", "s1"]);
    expect(mergeThread(have, [sent]).map((m) => m.id)).toEqual(["s4", "s3", "s2"]);
  });

  it("lets a later copy of a message replace the first (a delivery report)", () => {
    const pending = message({ id: "s1", status: "pending" });
    const delivered = message({ id: "s1", status: "delivered" });
    expect(mergeThread([pending], [delivered])[0].status).toBe("delivered");
  });
});
