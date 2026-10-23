import { parsePhoneNumber, type CountryCode } from "libphonenumber-js";
import type { SipAccount } from "../types/sip";

/**
 * Apply the account's dialing rules to what the user typed or picked, before
 * it is turned into a SIP URI:
 *
 * 1. Anything already addressed -- a `sip:user@host` or a bare `user@host` --
 *    is dialled exactly as written.
 * 2. A `sips?:` scheme with no host (system contacts are imported as
 *    `sip:5551234`) is just a bare number: the rules apply to the user part
 *    and `toSipUri` puts the scheme back.
 * 3. Feature codes -- anything starting with `*` or `#` -- are PBX commands,
 *    not phone numbers. No rule applies: a prefix must never turn `*97` into
 *    `9*97`.
 * 4. With autoCountryCode on, the number is parsed against `defaultCountry`
 *    and sent as E.164 digits (no `+`). A number that does not parse as valid
 *    is left alone.
 * 5. stripPrefix is removed from the front of the number, matched with or
 *    without a leading `+`.
 * 6. prependPrefix is added, unless the number still starts with `+`: an
 *    international number already has its full form, and an outside-line
 *    prefix in front of it would be dialled as digits.
 */
export function formatDialNumber(
  input: string,
  account?: Pick<SipAccount, "autoCountryCode" | "stripPrefix" | "prependPrefix"> | null,
  defaultCountry?: string,
): string {
  const trimmed = input.trim();
  if (!trimmed || trimmed.includes("@")) {
    return trimmed;
  }

  // A scheme without a host is a bare number (rule 2).
  let number = trimmed.replace(/^sips?:/i, "").trim();

  // Feature codes are passed through untouched (rule 3).
  if (/^[*#]/.test(number)) {
    return number;
  }

  number = number.replace(/[\s\-()]/g, "");

  // Auto-add the country code only if explicitly enabled (rule 4).
  if (account?.autoCountryCode && defaultCountry) {
    try {
      const parsed = parsePhoneNumber(number, defaultCountry as CountryCode);
      if (parsed?.isValid()) {
        number = parsed.number.replace("+", "");
      }
    } catch {
      // Keep input as-is if parsing fails
    }
  }

  // Dial plan / prefix manipulation (rules 5 and 6).
  const strip = account?.stripPrefix?.trim();
  if (strip) {
    if (number.startsWith("+") && number.slice(1).startsWith(strip)) {
      number = number.slice(1 + strip.length);
    } else if (number.startsWith(strip)) {
      number = number.slice(strip.length);
    }
  }

  const prepend = account?.prependPrefix?.trim();
  if (prepend && !number.startsWith("+")) {
    number = `${prepend}${number}`;
  }

  return number;
}

/**
 * The one way to turn a dial target into the URI that is actually sent:
 * the account's dialing rules, then `toSipUri` at the account's domain.
 *
 * Every place that starts a call -- the dialer, contacts, call history, "add
 * call", blind transfer, the voicemail button -- goes through here, so the
 * rules a user set up for the dialer hold wherever a number comes from.
 * `defaultCountry` is the app-wide setting from the store.
 */
export function dialUri(
  input: string,
  account: Pick<SipAccount, "domain" | "autoCountryCode" | "stripPrefix" | "prependPrefix"> | null | undefined,
  defaultCountry?: string,
): string {
  return toSipUri(formatDialNumber(input, account, defaultCountry), account?.domain ?? "");
}

/**
 * The SIP URI to dial for what the user typed or picked.
 *
 * Every place that starts a call used to build this itself, and they
 * disagreed: contacts imported from the system were saved as `sip:5551234`
 * with no host and dialled as-is, which the PBX read as a hostname; "Add
 * Call" sent the raw text; a typed "(555) 123-4567" went out with its
 * punctuation. One rule for all of them:
 *
 * - a full `sip:user@host` is used as it is;
 * - a `sip:` prefix without a host is treated like a bare number;
 * - anything with an `@` gets the scheme;
 * - a bare number loses its formatting and is addressed at `domain`.
 *
 * `#` is percent-encoded: it ends the user part of a URI, so `sip:123#@pbx`
 * is not what it looks like.
 */
export function toSipUri(input: string, domain: string): string {
  let target = input.trim();
  const scheme = target.match(/^sips?:/i);
  if (scheme) {
    const rest = target.slice(scheme[0].length);
    if (rest.includes("@")) return `${scheme[0].toLowerCase()}${rest}`;
    target = rest;
  }
  if (target.includes("@")) return `sip:${target}`;
  const user = target.replace(/[\s\-().]/g, "").replace(/#/g, "%23");
  return `sip:${user}@${domain}`;
}
