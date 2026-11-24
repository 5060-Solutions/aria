import type { Contact, SipAccount } from "../types/sip";

/** Which mailbox the backend should talk to: mirrors `voicemail::Mailbox`. */
export interface Mailbox {
  accountId: string;
  apiUrl: string;
  extension: string;
  domain: string;
}

/** One message as `voicemail_list` returns it. */
export interface VoicemailMessage {
  id: string;
  callerId: string;
  durationSecs: number;
  read: boolean;
  /** RFC 3339, as the PBX sent it. */
  createdAt: string;
  transcription: string | null;
}

/** The structured error the voicemail commands reject with. */
export interface VoicemailError {
  kind: "invalidUrl" | "unauthorized" | "noMailbox" | "network" | "status" | "malformed" | "noPassword";
  message: string;
}

export function isVoicemailError(e: unknown): e is VoicemailError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/**
 * The mailbox an account can read through the PBX API, or `null` when the
 * account has no API address: then the voicemail button dials in as before.
 */
export function mailboxFor(account: SipAccount | undefined | null): Mailbox | null {
  const apiUrl = account?.pbxApiUrl?.trim();
  if (!account || !apiUrl) return null;
  return { accountId: account.id, apiUrl, extension: account.username, domain: account.domain };
}

export function sortNewestFirst(messages: ReadonlyArray<VoicemailMessage>): VoicemailMessage[] {
  return [...messages].sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
}

export function unreadCount(messages: ReadonlyArray<VoicemailMessage>): number {
  return messages.filter((m) => !m.read).length;
}

/** `m:ss`, minutes unbounded: a voicemail is short, an hour is "60:00". */
export function formatVoicemailDuration(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const m = Math.floor(whole / 60);
  const s = whole % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** Today's messages show a time, older ones a date and time, in the UI locale. */
export function formatVoicemailTime(createdAt: string, now: Date = new Date()): string {
  const date = new Date(createdAt);
  if (Number.isNaN(date.getTime())) return "";
  const time = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (date.toDateString() === now.toDateString()) return time;
  return `${date.toLocaleDateString([], { month: "short", day: "numeric" })} · ${time}`;
}

const WITHHELD = new Set(["", "unknown", "anonymous", "restricted", "unavailable", "private"]);

function digits(value: string): string {
  return value.replace(/\D/g, "");
}

/** The user part of a contact URI, or the whole thing when it has no `@`. */
function uriUser(uri: string): string {
  return uri.replace(/^sips?:/i, "").split("@")[0] ?? "";
}

function sameNumber(a: string, b: string): boolean {
  if (!a || !b) return false;
  if (a === b) return true;
  // Trunk and national formats of one number differ in a country code.
  return a.length >= 7 && b.length >= 7 && (a.endsWith(b) || b.endsWith(a));
}

/**
 * What to call the caller: a matching contact's name, else the caller id the
 * PBX recorded, else `null` for a withheld or missing number (the UI shows
 * "Unknown caller").
 */
export function callerLabel(callerId: string, contacts: ReadonlyArray<Contact>): string | null {
  const id = callerId.trim();
  if (WITHHELD.has(id.toLowerCase())) return null;
  const wanted = digits(id);
  if (wanted) {
    const match = contacts.find((c) =>
      [uriUser(c.uri), c.phone ?? ""].some((candidate) => sameNumber(digits(candidate), wanted)),
    );
    if (match) return match.name;
  }
  return id;
}
