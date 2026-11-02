/**
 * Phone number normalization utilities.
 */

/**
 * Normalizes phone numbers for Mexico (10 digits) or international numbers:
 * - Strips punctuation, dashes, spaces, parentheses, dots, slashes
 * - Strips legacy Mexican domestic prefixes (044, 045, 01) down to 10 digits
 * - Strips Mexican international country codes (+52 1, +52, 52 1, 52, 0052) when followed by a 10-digit number
 * - Preserves SIP URIs, extension numbers, and other international numbers
 */
export function sanitizeMexicanPhoneNumber(raw: string): string {
  if (!raw) return "";
  const trimmed = raw.trim();
  if (trimmed.includes("@")) return trimmed; // SIP URI

  // Remove whitespace, dashes, parentheses, dots, slashes
  const digits = trimmed.replace(/[\s\-.()/]/g, "");

  // Check if it starts with +521 or +52
  if (digits.startsWith("+521") && digits.length === 14) {
    return digits.slice(4); // Keep 10 digits
  }
  if (digits.startsWith("+52") && digits.length === 13) {
    return digits.slice(3); // Keep 10 digits
  }
  if (digits.startsWith("0052") && digits.length === 14) {
    return digits.slice(4); // Keep 10 digits
  }
  if (digits.startsWith("521") && digits.length === 13) {
    return digits.slice(3); // Keep 10 digits
  }
  if (digits.startsWith("52") && digits.length === 12) {
    return digits.slice(2); // Keep 10 digits
  }
  // Check legacy domestic prefixes: 044, 045, 01
  if ((digits.startsWith("044") || digits.startsWith("045")) && digits.length === 13) {
    return digits.slice(3); // Keep 10 digits
  }
  if (digits.startsWith("01") && digits.length === 12) {
    return digits.slice(2); // Keep 10 digits
  }

  return digits;
}
