import { AsYouType, getCountries, getCountryCallingCode, parsePhoneNumberFromString, type CountryCode } from "libphonenumber-js";

/**
 * Display formatting for phone numbers: one place for the dialer, history,
 * contacts and messages. Formatting is for reading only. What is dialled is
 * `dialDigits(input)`, the digits as typed; the PBX's numbering plan
 * normalizes national numbers and handles extensions, star codes and the
 * outside-line prefix, so no country code is forced onto anything here.
 */

/** Characters people put between digits; never part of what is dialled. */
const SEPARATORS = /[\s().-]/g;

/** Below this many digits a national string is an extension or a short code. */
const MIN_NATIONAL_DIGITS = 7;

function digitCount(value: string): number {
  return value.replace(/\D/g, "").length;
}

/** Letters mean a name search, `*`/`#` a feature code: neither is a number. */
function isNumberLike(input: string): boolean {
  return /^\+?[\d\s().-]+$/.test(input);
}

/** What to dial for what is on the display: digits (with a leading `+`, `*` and `#`) and nothing else. */
export function dialDigits(input: string): string {
  const text = input.trim();
  if (!text || text.includes("@") || /[a-z]/i.test(text)) return text;
  return text.replace(SEPARATORS, "");
}

/**
 * The number as it should read while it is being typed. Extensions, short
 * codes and feature codes are left exactly as typed; a leading `+` formats as
 * the country its calling code names.
 */
export function formatAsYouType(input: string, defaultCountry: CountryCode): string {
  if (!input) return "";
  const text = input.replace(SEPARATORS, "");
  if (!isNumberLike(text)) return input;
  const international = text.startsWith("+");
  if (!international && digitCount(text) < MIN_NATIONAL_DIGITS) return text;
  try {
    return new AsYouType(international ? undefined : defaultCountry).input(text);
  } catch {
    return text;
  }
}

/**
 * A number in a list: national format for the default country's own calling
 * code, international otherwise. Anything that is not a valid number (an
 * extension, a star code, a short code, a name) is returned untouched.
 */
export function formatDisplayNumber(value: string, defaultCountry: CountryCode): string {
  const text = value.trim();
  if (!isNumberLike(text) || digitCount(text) < MIN_NATIONAL_DIGITS) return value;
  try {
    const parsed = parsePhoneNumberFromString(text, defaultCountry);
    if (!parsed?.isValid()) return value;
    return parsed.countryCallingCode === getCountryCallingCode(defaultCountry)
      ? parsed.formatNational()
      : parsed.formatInternational();
  } catch {
    return value;
  }
}

/**
 * The international reading of a valid national number, shown quietly under
 * what is typed ("+1 (800) 444-4444"); null for anything else, including a
 * number already typed with a `+`.
 */
export function dialHint(input: string, defaultCountry: CountryCode): string | null {
  const text = input.replace(SEPARATORS, "");
  if (!isNumberLike(text) || text.startsWith("+") || digitCount(text) < MIN_NATIONAL_DIGITS) return null;
  try {
    const parsed = parsePhoneNumberFromString(text, defaultCountry);
    if (!parsed?.isValid()) return null;
    return parsed.countryCallingCode === "1"
      ? `+1 ${parsed.formatNational()}`
      : parsed.formatInternational();
  } catch {
    return null;
  }
}

/** The country the device is set to, as a fallback when the PBX does not say. */
export function deviceCountry(language: string | undefined = typeof navigator === "undefined" ? undefined : navigator.language): CountryCode {
  const region = language?.split("-")[1]?.toUpperCase();
  const known = region && (getCountries() as string[]).includes(region);
  return known ? (region as CountryCode) : "US";
}

/**
 * The default country right now: the person's own pick, else what the PBX
 * said last time, else the device's region. Safe where there is no storage.
 */
export function storedDefaultCountry(): CountryCode {
  try {
    const stored = localStorage.getItem("aria_default_country") || localStorage.getItem("aria_pbx_country");
    if (stored) return stored as CountryCode;
  } catch {
    // No storage (tests, a locked-down webview): the device's region.
  }
  return deviceCountry();
}
