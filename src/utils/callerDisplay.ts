import { parsePhoneNumber, type CountryCode } from "libphonenumber-js";
import type { Contact } from "../types/sip";

/** The user part of a SIP URI (or the whole thing when it has no `@`). */
export function uriUser(uri: string): string {
  return uri.replace(/^sips?:/i, "").split("@")[0] ?? "";
}

function digits(value: string): string {
  return value.replace(/\D/g, "");
}

/** A number as a person reads it: international format when it parses, else as it came. */
export function formatNumber(user: string, defaultCountry: CountryCode): string {
  const d = digits(user);
  if (d.length < 7 || !/^\+?[\d\s().-]+$/.test(user)) return user;
  try {
    const parsed = parsePhoneNumber(user, defaultCountry);
    if (parsed?.isValid()) return parsed.formatInternational();
  } catch {
    // Not a phone number; shown as typed.
  }
  return user;
}

function sameNumber(a: string, b: string): boolean {
  if (!a || !b) return false;
  if (a === b) return true;
  return a.length >= 7 && b.length >= 7 && (a.endsWith(b) || b.endsWith(a));
}

/** The contact whose extension or number is `user`, if any. */
export function contactForNumber(user: string, contacts: ReadonlyArray<Contact>): Contact | null {
  const wanted = digits(user);
  if (!wanted) return null;
  return contacts.find((c) => [uriUser(c.uri), c.phone ?? ""].some((v) => sameNumber(digits(v), wanted))) ?? null;
}

export interface CallerDisplay {
  /** The big line: a name when one is known, else the number. */
  primary: string;
  /** The small line under it: the number when the big line is a name. */
  secondary: string | null;
}

/**
 * What the call screen says about the far end. Never the host: a SIP
 * address is for the Diagnostics window, not for the person on the call.
 * A display name from the PBX wins, then a matching contact, then the
 * number formatted for reading; an extension or a withheld id stays as it
 * is.
 */
export function callerDisplay(
  call: { remoteUri: string; remoteName?: string | null },
  contacts: ReadonlyArray<Contact>,
  defaultCountry: CountryCode,
): CallerDisplay {
  const user = uriUser(call.remoteUri.trim());
  const number = formatNumber(user, defaultCountry);
  const sent = call.remoteName?.trim() ?? "";
  // A "name" that is the number again (what some PBXes send) is not a name.
  const name =
    sent && digits(sent) !== digits(user) ? sent : (contactForNumber(user, contacts)?.name ?? null);
  if (name) return { primary: name, secondary: number };
  return { primary: number, secondary: null };
}

/**
 * Contacts the dialer offers for what was typed: by name once there are
 * letters, by digits otherwise, favourites first. Nothing for an empty box.
 */
export function dialerMatches(input: string, contacts: ReadonlyArray<Contact>, limit = 5): Contact[] {
  const q = input.trim().toLowerCase();
  if (!q) return [];
  const qDigits = digits(q);
  const byName = /[a-z]/i.test(q);
  const hits = contacts.filter((c) => {
    if (byName) return c.name.toLowerCase().includes(q);
    return qDigits.length > 0 && [uriUser(c.uri), c.phone ?? ""].some((v) => digits(v).includes(qDigits));
  });
  return hits
    .sort((a, b) => Number(b.favorite) - Number(a.favorite) || a.name.localeCompare(b.name))
    .slice(0, limit);
}
