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
