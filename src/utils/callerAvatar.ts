/**
 * What the call screen's avatar shows for the other party: initials for a
 * real name, or null for anything that is only a dial string (a number, a
 * feature code like *97, a SIP URI). Cutting the first two characters out of
 * a dial string gave "*9" for *97 and "+1" for every US number, which reads
 * as a mistake; the caller shows an icon instead.
 */
export function callerInitials(remoteName: string | null | undefined): string | null {
  const name = remoteName?.trim();
  if (!name) return null;
  // A display name that is really an address or a number is not a name.
  if (/^sips?:/i.test(name) || name.includes("@")) return null;

  const letters = name
    .split(/\s+/)
    .map((word) => word.match(/\p{L}/u)?.[0])
    .filter((c): c is string => c !== undefined);
  if (letters.length === 0) return null;

  const first = letters[0];
  const last = letters.length > 1 ? letters[letters.length - 1] : "";
  return (first + last).toLocaleUpperCase();
}
