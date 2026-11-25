import { describe, expect, it } from "vitest";
import { callIdTag, messageCategory, parseStatus, splitSipMessage } from "./sipMessage";

const INVITE =
  "INVITE sip:1002@test.ariaroute.com SIP/2.0\r\n" +
  "Via: SIP/2.0/TLS 10.0.0.2:5061;branch=z9hG4bK-1\r\n" +
  "Call-ID: 7b5f63ed-046b-461e-a602-50707569f8b7\r\n" +
  "CSeq: 1 INVITE\r\n" +
  "Content-Type: application/sdp\r\n" +
  "Content-Length: 31\r\n" +
  "\r\n" +
  "v=0\r\nm=audio 40000 RTP/AVP 0\r\n";

describe("splitSipMessage", () => {
  it("separates the start line, the headers and the body", () => {
    const m = splitSipMessage(INVITE);
    expect(m.startLine).toBe("INVITE sip:1002@test.ariaroute.com SIP/2.0");
    expect(m.headers).toEqual([
      ["Via", "SIP/2.0/TLS 10.0.0.2:5061;branch=z9hG4bK-1"],
      ["Call-ID", "7b5f63ed-046b-461e-a602-50707569f8b7"],
      ["CSeq", "1 INVITE"],
      ["Content-Type", "application/sdp"],
      ["Content-Length", "31"],
    ]);
    expect(m.body).toBe("v=0\r\nm=audio 40000 RTP/AVP 0\r\n");
  });

  it("copes with a message that has no body and bare LF line ends", () => {
    const m = splitSipMessage("OPTIONS sip:pbx SIP/2.0\nCSeq: 2 OPTIONS\n\n");
    expect(m.startLine).toBe("OPTIONS sip:pbx SIP/2.0");
    expect(m.headers).toEqual([["CSeq", "2 OPTIONS"]]);
    expect(m.body).toBe("");
  });
});

describe("parseStatus", () => {
  it("reads a response's code and a request's method", () => {
    expect(parseStatus("SIP/2.0 401 Unauthorized")).toEqual({ kind: "response", code: 401, text: "Unauthorized" });
    expect(parseStatus("INVITE sip:1002@pbx SIP/2.0")).toEqual({ kind: "request", method: "INVITE", target: "sip:1002@pbx" });
    expect(parseStatus("Assistant placed a call")).toEqual({ kind: "other" });
  });
});

describe("messageCategory", () => {
  it("files a message by the method in its CSeq", () => {
    expect(messageCategory(INVITE)).toBe("calls");
    expect(messageCategory("SIP/2.0 200 OK\r\nCSeq: 3 REGISTER\r\n\r\n")).toBe("register");
    expect(messageCategory("OPTIONS sip:pbx SIP/2.0\r\nCSeq: 1 OPTIONS\r\n\r\n")).toBe("keepalive");
    expect(messageCategory("SUBSCRIBE sip:pbx SIP/2.0\r\nCSeq: 1 SUBSCRIBE\r\n\r\n")).toBe("other");
    expect(messageCategory("no cseq at all")).toBe("other");
  });
});

describe("callIdTag", () => {
  it("is a short, stable tag for a Call-ID with a colour index", () => {
    const a = callIdTag("7b5f63ed-046b-461e-a602-50707569f8b7");
    expect(a.text).toHaveLength(4);
    expect(a).toEqual(callIdTag("7b5f63ed-046b-461e-a602-50707569f8b7"));
    expect(a.hue).toBeGreaterThanOrEqual(0);
    expect(a.hue).toBeLessThan(360);
    expect(callIdTag("another-call-id").text).not.toBe(a.text);
  });
});
