/** Pure helpers for showing a SIP message in the Diagnostics window. */

export interface SplitMessage {
  startLine: string;
  /** Header name and value, in order; a folded header is one entry. */
  headers: [string, string][];
  body: string;
}

/** Start line, headers and body of a raw SIP message (CRLF or LF). */
export function splitSipMessage(raw: string): SplitMessage {
  const normalized = raw.replace(/\r\n/g, "\n");
  const blank = normalized.indexOf("\n\n");
  const head = blank === -1 ? normalized : normalized.slice(0, blank);
  const body = blank === -1 ? "" : normalized.slice(blank + 2);
  const lines = head.split("\n");
  const startLine = lines.shift()?.trim() ?? "";
  const headers: [string, string][] = [];
  for (const line of lines) {
    if (line === "") continue;
    if (/^[ \t]/.test(line) && headers.length > 0) {
      // RFC 3261 §7.3.1: a line starting with whitespace continues the header above.
      const last = headers[headers.length - 1];
      last[1] = `${last[1]} ${line.trim()}`;
      continue;
    }
    const colon = line.indexOf(":");
    if (colon === -1) {
      headers.push([line.trim(), ""]);
    } else {
      headers.push([line.slice(0, colon).trim(), line.slice(colon + 1).trim()]);
    }
  }
  return { startLine, headers, body: body.replace(/\n/g, "\r\n") };
}

export type StartLine =
  | { kind: "response"; code: number; text: string }
  | { kind: "request"; method: string; target: string }
  | { kind: "other" };

/** What a start line is: a response with its code, a request with its method, or neither. */
export function parseStatus(startLine: string): StartLine {
  const response = startLine.match(/^SIP\/2\.0\s+(\d{3})\s*(.*)$/);
  if (response) return { kind: "response", code: Number(response[1]), text: response[2].trim() };
  const request = startLine.match(/^([A-Z]+)\s+(\S+)\s+SIP\/2\.0$/);
  if (request) return { kind: "request", method: request[1], target: request[2] };
  return { kind: "other" };
}

export type MessageCategory = "calls" | "register" | "keepalive" | "other";

const CALL_METHODS = new Set(["INVITE", "BYE", "ACK", "CANCEL", "REFER", "INFO", "UPDATE", "PRACK"]);

/** Which filter a message belongs to, by the method its CSeq names. */
export function messageCategory(raw: string): MessageCategory {
  const cseq = raw.match(/^CSeq:\s*\d+\s+(\w+)/im);
  const method = cseq?.[1]?.toUpperCase();
  if (!method) return "other";
  if (CALL_METHODS.has(method)) return "calls";
  if (method === "REGISTER") return "register";
  if (method === "OPTIONS") return "keepalive";
  return "other";
}

/**
 * A four-character tag and a hue for a Call-ID, stable across renders, so
 * the messages of one dialog can be told apart at a glance and clicked to
 * filter.
 */
export function callIdTag(callId: string): { text: string; hue: number } {
  let h = 2166136261;
  for (let i = 0; i < callId.length; i++) {
    h ^= callId.charCodeAt(i);
    h = Math.imul(h, 16777619) >>> 0;
  }
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  let text = "";
  let v = h;
  for (let i = 0; i < 4; i++) {
    text += alphabet[v % alphabet.length];
    v = Math.floor(v / alphabet.length);
  }
  return { text, hue: h % 360 };
}
