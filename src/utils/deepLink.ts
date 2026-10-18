/**
 * The dial target a `tel:` or `sip:` deep link asks for, or null if the link
 * is not one we handle or is malformed.
 *
 * Decoding can throw on a bad percent sequence, which used to abort handling
 * of every link in the batch. Line breaks and other control characters are
 * stripped: decoded into the dialer and then dialled, they would be written
 * into the INVITE's headers. The backend rejects them too; this just keeps
 * them out of the input field.
 */
export function deepLinkTarget(url: string): string | null {
  const scheme = url.startsWith("tel:") ? "tel:" : url.startsWith("sip:") ? "sip:" : null;
  if (!scheme) return null;
  let target: string;
  try {
    target = decodeURIComponent(url.slice(scheme.length));
  } catch {
    return null;
  }
  target = Array.from(target)
    .filter((c) => {
      const code = c.charCodeAt(0);
      return code > 0x20 && code !== 0x7f && !/\s/.test(c);
    })
    .join("");
  return target.length > 0 ? target : null;
}
