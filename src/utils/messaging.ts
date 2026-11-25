import { parsePhoneNumber, type CountryCode } from "libphonenumber-js";
import type { Contact } from "../types/sip";
import { contactForNumber, formatNumber } from "./callerDisplay";

/** A messaging number the extension may use: mirrors `messaging::Inbox`. */
export interface Inbox {
  didId: string;
  number: string;
  /** `false` is a number an admin still reads but nobody sends from. */
  enabled: boolean;
  lastInboundAt: string | null;
}

export interface Conversation {
  remoteNumber: string;
  messageCount: number;
  lastBody: string;
  lastDirection: "inbound" | "outbound" | string;
  lastStatus: string;
  /** RFC 3339, as the PBX sent it. */
  lastAt: string;
  lastHasMedia: boolean;
  optedOut: boolean;
}

export interface SmsAttachment {
  id: string;
  contentType: string;
  sizeBytes: number;
}

export interface SmsMessage {
  id: string;
  direction: "inbound" | "outbound" | string;
  remoteNumber: string;
  body: string;
  /** `pending`, `sent`, `delivered`, `failed` or `received`. */
  status: string;
  autoReply: boolean;
  errorMessage: string | null;
  createdAt: string;
  attachments: SmsAttachment[];
}

export interface Thread {
  remoteNumber: string;
  optedOut: boolean;
  /** Newest first. */
  messages: SmsMessage[];
}

/** The most segments one text may span (the PBX refuses more). */
export const MAX_SEGMENTS = 10;

export interface Segments {
  segments: number;
  /** Septets (GSM-7) or UTF-16 units (UCS-2): what the carrier counts. */
  chars: number;
  perSegment: number;
  encoding: "gsm7" | "ucs2";
}

// GSM 03.38 basic character set, and the characters its escape table
// reaches (each of those costs two septets).
const GSM_BASIC =
  "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà";
const GSM_EXTENDED = "\f^{}\\[~]|€";

/**
 * How a text will be sent: GSM-7 (160, then 153 a part) when every
 * character is in that alphabet, otherwise UCS-2 (70, then 67), the same
 * arithmetic the PBX applies before it refuses more than ten parts.
 */
export function smsSegments(body: string): Segments {
  if (!body.trim()) return { segments: 0, chars: 0, perSegment: 160, encoding: "gsm7" };
  let septets = 0;
  let gsm = true;
  for (const ch of body) {
    if (GSM_BASIC.includes(ch)) septets += 1;
    else if (GSM_EXTENDED.includes(ch)) septets += 2;
    else {
      gsm = false;
      break;
    }
  }
  if (gsm) {
    const perSegment = septets <= 160 ? 160 : 153;
    return { segments: Math.ceil(septets / perSegment), chars: septets, perSegment, encoding: "gsm7" };
  }
  const units = body.length;
  const perSegment = units <= 70 ? 70 : 67;
  return { segments: Math.ceil(units / perSegment), chars: units, perSegment, encoding: "ucs2" };
}

/** Strict E.164 for what was typed, or `null` when it is not a phone number. */
export function normalizeRecipient(input: string, defaultCountry: CountryCode): string | null {
  const trimmed = input.trim();
  if (!trimmed || !/^\+?[\d\s().-]+$/.test(trimmed)) return null;
  try {
    const parsed = parsePhoneNumber(trimmed, defaultCountry);
    return parsed?.isValid() ? parsed.number : null;
  } catch {
    return null;
  }
}

/** A conversation's title: the contact's name over the number, or the number. */
export function conversationName(
  remoteNumber: string,
  contacts: ReadonlyArray<Contact>,
  defaultCountry: CountryCode,
): { primary: string; secondary: string | null } {
  const shown = formatNumber(remoteNumber, defaultCountry);
  const contact = contactForNumber(remoteNumber, contacts);
  return contact ? { primary: contact.name, secondary: shown } : { primary: shown, secondary: null };
}

export function sortByLastAt(conversations: ReadonlyArray<Conversation>): Conversation[] {
  return [...conversations].sort((a, b) => Date.parse(b.lastAt) - Date.parse(a.lastAt));
}

/**
 * Conversations with an inbound text newer than the last time they were
 * opened. The PBX keeps no read flag on texts, so `seen` is this app's own
 * record: remote number to the `lastAt` it showed when the thread was open.
 */
export function unseenCount(conversations: ReadonlyArray<Conversation>, seen: Record<string, string>): number {
  return conversations.filter((c) => {
    if (c.lastDirection !== "inbound") return false;
    const last = seen[c.remoteNumber];
    return !last || Date.parse(c.lastAt) > Date.parse(last);
  }).length;
}

/** `have` plus `more` (an older page, a new send, a refreshed copy), by id, newest first. */
export function mergeThread(have: ReadonlyArray<SmsMessage>, more: ReadonlyArray<SmsMessage>): SmsMessage[] {
  const byId = new Map<string, SmsMessage>();
  for (const m of have) byId.set(m.id, m);
  for (const m of more) byId.set(m.id, m);
  return [...byId.values()].sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
}
