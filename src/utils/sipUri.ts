import { parsePhoneNumber, type CountryCode } from "libphonenumber-js";
import type { SipAccount } from "../types/sip";

/**
 * Format and prepare a dialed number according to account rules:
 * 1. If it's a full SIP URI or contains '@', return as-is.
 * 2. If autoCountryCode is enabled on the account, parse with defaultCountry
 *    and format as E.164 (without '+').
 * 3. Apply stripPrefix if the number starts with it (handles matching with or without leading '+').
 * 4. Apply prependPrefix if configured.
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

  let number = trimmed.replace(/[\s\-()]/g, "");

  // Option 1: Auto-add country code only if explicitly enabled
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

  // Option 3: Dial plan / prefix manipulation
  const strip = account?.stripPrefix?.trim();
  if (strip) {
    if (number.startsWith("+") && number.slice(1).startsWith(strip)) {
      number = number.slice(1 + strip.length);
    } else if (number.startsWith(strip)) {
      number = number.slice(strip.length);
    }
  }

  const prepend = account?.prependPrefix?.trim();
  if (prepend) {
    number = `${prepend}${number}`;
  }

  return number;
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
