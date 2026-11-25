import { parsePhoneNumberFromString, type CountryCode } from "libphonenumber-js";
import type { SipAccount } from "../types/sip";
import type { Contact } from "../types/sip";
import { callerLabel, mailboxFor, type Mailbox } from "./voicemail";

/** Business texting reaches the PBX the way visual voicemail does. */
export const messagingMailboxFor: (account: SipAccount | undefined | null) => Mailbox | null = mailboxFor;

/** A number the extension may text from (`GET .../messaging/inboxes`). */
export interface Inbox {
  did_id: string;
  number: string;
  last_inbound_at: string | null;
  /** Off for a number an admin can still read; nothing is sent from it. */
  enabled: boolean;
}

export interface Conversation {
  remote_number: string;
  message_count: number;
  last_body: string;
  last_direction: "inbound" | "outbound";
  last_status: MessageStatus;
  last_at: string;
  last_has_media: boolean;
  opted_out: boolean;
}

export type MessageStatus = "pending" | "sent" | "delivered" | "failed" | "received";

export interface Attachment {
  id: string;
  content_type: string;
  size_bytes: number;
}

export interface SmsMessage {
  id: string;
  direction: "inbound" | "outbound";
  remote_number: string;
  body: string;
  status: MessageStatus;
  error_message: string | null;
  created_at: string;
  attachments: Attachment[];
}

export interface Thread {
  remote_number: string;
  opted_out: boolean;
  /** Newest first, as the PBX sends them. */
  messages: SmsMessage[];
}

/** What the messaging commands reject with. */
export interface MessagingError {
  kind: string;
  message: string;
  status?: number;
  /** What the PBX objected to in a send: `to`, `body`, `Idempotency-Key`. */
  field?: string;
}

export function isMessagingError(e: unknown): e is MessagingError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

/** The sentence to show: the PBX's own when it gave one. */
export function messagingErrorText(e: unknown): string {
  if (isMessagingError(e)) return e.message;
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * Whether a failed send got a settled answer from the PBX (a 4xx). After one
 * the next attempt is a new request and gets a new idempotency key. After a
 * 5xx, or no answer at all (the network dropped), the same key is kept, so a
 * retry of a send that did go through returns that message and texts nobody
 * twice.
 */
export function wasAnswered(e: unknown): boolean {
  return isMessagingError(e) && e.kind === "rejected";
}

/**
 * Whether the PBX itself answered, 4xx or 5xx: what a refresh can show. A
 * carrier that refused a text (502) leaves it in the thread as failed, but the
 * answer is not a settled one, so the retry keeps its key (`wasAnswered`).
 */
export function reachedServer(e: unknown): boolean {
  return isMessagingError(e) && (e.kind === "rejected" || e.kind === "server");
}

// ── Segments ────────────────────────────────────────────────────────────────

/** The most segments the PBX accepts in one message. */
export const MAX_SEGMENTS = 10;
/** The longest body it reads. */
export const MAX_BODY_CHARS = 1600;

// GSM 03.38, as the PBX counts it (carrier_adapters/messaging.rs).
const GSM7_BASIC =
  "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà";
const GSM7_EXTENSION = "^{}\\[~]|€\f";

export interface SegmentCount {
  encoding: "gsm7" | "ucs2";
  /** Septets (GSM-7) or UTF-16 units (UCS-2). */
  units: number;
  segments: number;
  /** Units left in the segment in use. */
  remaining: number;
  overLimit: boolean;
}

/**
 * How many SMS segments `body` takes: GSM-7 (160 alone, 153 per part) when
 * every character is in the GSM alphabet, UCS-2 (70, 67) otherwise; extension
 * characters (`€`, `[`, ...) cost two septets. The same rule the PBX enforces,
 * so the counter and the refusal agree.
 */
export function countSegments(body: string): SegmentCount {
  let septets = 0;
  let gsm = true;
  for (const ch of body) {
    if (GSM7_BASIC.includes(ch)) septets += 1;
    else if (GSM7_EXTENSION.includes(ch)) septets += 2;
    else {
      gsm = false;
      break;
    }
  }
  const units = gsm ? septets : body.length; // `length` counts UTF-16 units
  const [single, part] = gsm ? [160, 153] : [70, 67];
  const segments = units === 0 ? 0 : units <= single ? 1 : Math.ceil(units / part);
  const capacity = segments <= 1 ? single : segments * part;
  return {
    encoding: gsm ? "gsm7" : "ucs2",
    units,
    segments,
    remaining: segments === 0 ? single : capacity - units,
    overLimit: segments > MAX_SEGMENTS || body.length > MAX_BODY_CHARS,
  };
}

// ── Numbers ─────────────────────────────────────────────────────────────────

/** What the person typed as strict E.164 (`+13035550100`), or `null`. */
export function toE164(input: string, defaultCountry: string): string | null {
  const text = input.trim();
  if (!text) return null;
  const parsed = parsePhoneNumberFromString(text, defaultCountry as CountryCode);
  return parsed?.isValid() ? parsed.number : null;
}

/** `+13035550100` as `+1 303 555 0100`; anything unparseable as it is. */
export function formatNumber(number: string): string {
  return parsePhoneNumberFromString(number)?.formatInternational() ?? number;
}

// ── Unread ──────────────────────────────────────────────────────────────────

/** The PBX keeps no read state for texts; the app remembers when each thread was last opened. */
export type ReadMarkers = Record<string, string>;

export function threadKey(accountId: string, didId: string, number: string): string {
  return `${accountId}|${didId}|${number}`;
}

/**
 * Whether the conversation's latest text is inbound and newer than the last
 * time the thread was opened. Nothing that predates `baseline` (when this
 * account first loaded texts here) counts: a first run does not light up a
 * year of history.
 */
export function isUnread(c: Conversation, readAt: string | undefined, baseline: string): boolean {
  if (c.last_direction !== "inbound") return false;
  const last = Date.parse(c.last_at);
  if (Number.isNaN(last)) return false;
  const floor = Math.max(Date.parse(readAt ?? "") || 0, Date.parse(baseline) || 0);
  return last > floor;
}

// ── Attachments ─────────────────────────────────────────────────────────────

export const MAX_ATTACHMENT_BYTES = 5 * 1024 * 1024;
export const ATTACHMENT_TYPES = ["image/jpeg", "image/png", "image/gif", "video/mp4", "application/pdf"];

/** Why a file cannot be sent, or `null`. The PBX types it by its bytes and checks again. */
export function attachmentProblem(file: { type: string; size: number }): "type" | "size" | null {
  if (!ATTACHMENT_TYPES.includes(file.type)) return "type";
  if (file.size > MAX_ATTACHMENT_BYTES) return "size";
  return null;
}

export function isImage(contentType: string): boolean {
  return contentType.startsWith("image/");
}

// ── Status ──────────────────────────────────────────────────────────────────

/** A message the person sent that has not been delivered or has failed, for the line under it. */
export function showsStatus(m: Pick<SmsMessage, "direction">): boolean {
  return m.direction === "outbound";
}

/** The thread oldest first, for display. */
export function oldestFirst(messages: ReadonlyArray<SmsMessage>): SmsMessage[] {
  return [...messages].sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at));
}

/** Conversations newest first. */
export function newestConversationsFirst(items: ReadonlyArray<Conversation>): Conversation[] {
  return [...items].sort((a, b) => Date.parse(b.last_at) - Date.parse(a.last_at));
}

/** A fresh idempotency key for one send. */
export function newIdempotencyKey(): string {
  return crypto.randomUUID();
}

/** A text's first line for a list row or a notification. */
export function previewOf(body: string, hasMedia: boolean, pictureLabel: string): string {
  const text = body.trim().replace(/\s+/g, " ");
  if (text) return text;
  return hasMedia ? pictureLabel : "";
}

/**
 * Conversations whose latest inbound text arrived since `prev` was read:
 * what a refresh found that the last one did not. `prev` is `undefined` on the
 * first load, which finds nothing (opening the app is not "a new text").
 */
export function newInbound(prev: ReadonlyArray<Conversation> | undefined, next: ReadonlyArray<Conversation>): Conversation[] {
  if (!prev) return [];
  const known = new Map(prev.map((c) => [c.remote_number, Date.parse(c.last_at)]));
  return next.filter((c) => {
    if (c.last_direction !== "inbound") return false;
    const before = known.get(c.remote_number);
    const at = Date.parse(c.last_at);
    return Number.isFinite(at) && (before === undefined || at > before);
  });
}

/** What to call someone: a contact's name, else their formatted number. */
export function personLabel(number: string, contacts: ReadonlyArray<Contact>): string {
  const label = callerLabel(number, contacts);
  return !label || label === number ? formatNumber(number) : label;
}

/**
 * Whether the Messages item belongs in the nav: not until the PBX has
 * answered, and not once it has said there is no number to text from. A failed
 * load keeps it, so the error is somewhere to be seen.
 */
export function showMessagesNav(state: { loaded: boolean; error: unknown; inboxes: ReadonlyArray<unknown> } | undefined): boolean {
  if (!state || !state.loaded) return false;
  return state.error !== null || state.inboxes.length > 0;
}

/**
 * A number safe to hand to the dialer: strict E.164. A short code, a star
 * code, a `#`, an `@` or a SIP URI that a sender could put in a text's number
 * never reaches `dialUri`.
 */
export function isDialable(number: string): boolean {
  return /^\+[1-9]\d{6,14}$/.test(number);
}

// ── Drafts ──────────────────────────────────────────────────────────────────

/** What is being written in one thread, kept across the view coming and going. */
export interface Draft {
  body: string;
  attachment: Attachment | null;
  /** Idempotency key of the send this draft would make. */
  key: string;
  sending: boolean;
  /** The PBX's sentence for the last failed send. */
  error: string | null;
}

export function emptyDraft(): Draft {
  return { body: "", attachment: null, key: newIdempotencyKey(), sending: false, error: null };
}

/** An edit: anything that changes what would be sent is a new request, so a new key. */
export function editDraft(d: Draft, patch: { body?: string; attachment?: Attachment | null }): Draft {
  const body = patch.body ?? d.body;
  const attachment = patch.attachment === undefined ? d.attachment : patch.attachment;
  const changed = body !== d.body || attachment?.id !== d.attachment?.id;
  return { ...d, body, attachment, key: changed ? newIdempotencyKey() : d.key, error: changed ? null : d.error };
}

/** The draft after a send failed with `e`. */
export function failedDraft(d: Draft, e: unknown): Draft {
  return { ...d, sending: false, error: messagingErrorText(e), key: wasAnswered(e) ? newIdempotencyKey() : d.key };
}

/** What is sent for a draft: the trimmed body, the one thing the counter must count. */
export function sendable(d: Pick<Draft, "body">): string {
  return d.body.trim();
}
