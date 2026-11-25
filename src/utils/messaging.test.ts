import { describe, expect, it } from "vitest";
import {
  attachmentProblem,
  countSegments,
  isUnread,
  newInbound,
  messagingErrorText,
  newestConversationsFirst,
  oldestFirst,
  personLabel,
  previewOf,
  showMessagesNav,
  toE164,
  wasAnswered,
  type Conversation,
  type SmsMessage,
} from "./messaging";

function conversation(over: Partial<Conversation> = {}): Conversation {
  return {
    remote_number: "+13035550100",
    message_count: 2,
    last_body: "hi",
    last_direction: "inbound",
    last_status: "received",
    last_at: "2026-11-25T12:00:00Z",
    last_has_media: false,
    opted_out: false,
    ...over,
  };
}

describe("countSegments", () => {
  it("matches the PBX for GSM-7 text", () => {
    expect(countSegments("").segments).toBe(0);
    expect(countSegments("a".repeat(160)).segments).toBe(1);
    expect(countSegments("a".repeat(161)).segments).toBe(2);
    expect(countSegments("a".repeat(153 * 10)).segments).toBe(10);
    expect(countSegments("a".repeat(153 * 10)).overLimit).toBe(false);
    expect(countSegments("a".repeat(153 * 10 + 1)).overLimit).toBe(true);
  });

  it("counts extension characters as two septets", () => {
    // 80 euro signs are 160 septets: one segment. 81 are two.
    expect(countSegments("€".repeat(80))).toMatchObject({ encoding: "gsm7", units: 160, segments: 1 });
    expect(countSegments("€".repeat(81)).segments).toBe(2);
  });

  it("switches to UCS-2 for any character outside the GSM alphabet", () => {
    expect(countSegments("a".repeat(70))).toMatchObject({ encoding: "gsm7", segments: 1 });
    const unicode = countSegments("a".repeat(69) + "日");
    expect(unicode).toMatchObject({ encoding: "ucs2", units: 70, segments: 1 });
    expect(countSegments("日".repeat(71))).toMatchObject({ encoding: "ucs2", segments: 2 });
    expect(countSegments("日".repeat(67 * 10)).overLimit).toBe(false);
    expect(countSegments("日".repeat(67 * 10 + 1)).overLimit).toBe(true);
  });

  it("counts an emoji as two UTF-16 units", () => {
    expect(countSegments("😀")).toMatchObject({ encoding: "ucs2", units: 2, segments: 1 });
  });

  it("reports what is left in the segment in use", () => {
    expect(countSegments("").remaining).toBe(160);
    expect(countSegments("abc").remaining).toBe(157);
    expect(countSegments("a".repeat(161)).remaining).toBe(2 * 153 - 161);
  });

  it("refuses a body past 1600 characters even if it fits in segments", () => {
    expect(countSegments("a".repeat(1601)).overLimit).toBe(true);
  });
});

describe("toE164", () => {
  it("reads a national number under the default country", () => {
    expect(toE164("(303) 555-0100", "US")).toBe("+13035550100");
    expect(toE164("303-555-0100", "US")).toBe("+13035550100");
  });
  it("keeps a full international number", () => {
    expect(toE164("+1 303 555 0100", "GB")).toBe("+13035550100");
  });
  it("is null for what is not a phone number", () => {
    expect(toE164("", "US")).toBeNull();
    expect(toE164("hello", "US")).toBeNull();
    expect(toE164("123", "US")).toBeNull();
  });
});

describe("isUnread", () => {
  const baseline = "2026-11-01T00:00:00Z";
  it("is unread when the last text is inbound and newer than the last open", () => {
    expect(isUnread(conversation(), undefined, baseline)).toBe(true);
    expect(isUnread(conversation(), "2026-11-25T11:00:00Z", baseline)).toBe(true);
  });
  it("is read once the thread was opened after it", () => {
    expect(isUnread(conversation(), "2026-11-25T12:00:01Z", baseline)).toBe(false);
  });
  it("is never unread for what we sent", () => {
    expect(isUnread(conversation({ last_direction: "outbound" }), undefined, baseline)).toBe(false);
  });
  it("ignores history from before the baseline", () => {
    expect(isUnread(conversation({ last_at: "2026-10-01T00:00:00Z" }), undefined, baseline)).toBe(false);
  });
});

describe("attachmentProblem", () => {
  it("accepts what the PBX accepts", () => {
    expect(attachmentProblem({ type: "image/jpeg", size: 1000 })).toBeNull();
    expect(attachmentProblem({ type: "application/pdf", size: 5 * 1024 * 1024 })).toBeNull();
  });
  it("names the problem", () => {
    expect(attachmentProblem({ type: "image/webp", size: 10 })).toBe("type");
    expect(attachmentProblem({ type: "image/png", size: 5 * 1024 * 1024 + 1 })).toBe("size");
  });
});

describe("errors", () => {
  it("shows the PBX's own sentence", () => {
    expect(messagingErrorText({ kind: "rejected", message: "That person opted out (STOP).", status: 422, field: "to" })).toBe(
      "That person opted out (STOP).",
    );
    expect(messagingErrorText("boom")).toBe("boom");
  });
  it("tells an answered failure from a silent one", () => {
    expect(wasAnswered({ kind: "rejected", message: "x" })).toBe(true);
    expect(wasAnswered({ kind: "network", message: "x" })).toBe(false);
    expect(wasAnswered(new Error("x"))).toBe(false);
  });
});

describe("ordering and preview", () => {
  it("shows a thread oldest first and conversations newest first", () => {
    const m = (id: string, at: string) => ({ id, created_at: at }) as SmsMessage;
    expect(oldestFirst([m("b", "2026-11-25T02:00:00Z"), m("a", "2026-11-25T01:00:00Z")]).map((x) => x.id)).toEqual(["a", "b"]);
    const cs = newestConversationsFirst([
      conversation({ remote_number: "old", last_at: "2026-11-24T00:00:00Z" }),
      conversation({ remote_number: "new", last_at: "2026-11-25T00:00:00Z" }),
    ]);
    expect(cs.map((c) => c.remote_number)).toEqual(["new", "old"]);
  });
  it("previews the text, else the picture", () => {
    expect(previewOf("  hello\n  there ", false, "Picture")).toBe("hello there");
    expect(previewOf("", true, "Picture")).toBe("Picture");
    expect(previewOf("", false, "Picture")).toBe("");
  });
});

describe("newInbound", () => {
  it("finds nothing on the first load", () => {
    expect(newInbound(undefined, [conversation()])).toEqual([]);
  });
  it("finds a new conversation and a newer text in an old one", () => {
    const old = conversation({ remote_number: "+1", last_at: "2026-11-25T10:00:00Z" });
    const next = [
      conversation({ remote_number: "+1", last_at: "2026-11-25T10:00:01Z" }),
      conversation({ remote_number: "+2", last_at: "2026-11-25T09:00:00Z" }),
    ];
    expect(newInbound([old], next).map((c) => c.remote_number)).toEqual(["+1", "+2"]);
  });
  it("ignores unchanged threads and our own texts", () => {
    const same = conversation({ remote_number: "+1" });
    expect(newInbound([same], [same])).toEqual([]);
    expect(newInbound([same], [conversation({ remote_number: "+1", last_direction: "outbound", last_at: "2026-11-26T00:00:00Z" })])).toEqual([]);
  });
});

describe("showMessagesNav", () => {
  it("waits for the PBX, then follows whether there is a number to text from", () => {
    expect(showMessagesNav(undefined)).toBe(false);
    expect(showMessagesNav({ loaded: false, error: null, inboxes: [] })).toBe(false);
    expect(showMessagesNav({ loaded: true, error: null, inboxes: [] })).toBe(false);
    expect(showMessagesNav({ loaded: true, error: null, inboxes: [{}] })).toBe(true);
    expect(showMessagesNav({ loaded: true, error: { kind: "network" }, inboxes: [] })).toBe(true);
  });
});

describe("personLabel", () => {
  it("is the contact's name, else the formatted number", () => {
    const contacts = [{ name: "Dana", uri: "sip:x@y", phone: "+13035550100" }] as never;
    expect(personLabel("+13035550100", contacts)).toBe("Dana");
    expect(personLabel("+13035550111", contacts)).toBe("+1 303 555 0111");
  });
});
