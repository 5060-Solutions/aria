/**
 * Checks for SIP settings that arrive from outside (a QR code), matching the
 * backend's checks on setup-link replies (`src-tauri/src/provision.rs`). They
 * end up in SIP headers, so anything that could break one is refused.
 */

/** A DNS name or IPv4 address, no port. */
export function isHost(s: string): boolean {
  return s.length > 0 && s.length <= 253 && /^[A-Za-z0-9.-]+$/.test(s) && !/^[.-]/.test(s);
}

/** The user part of a SIP URI, without anything that would need escaping. */
export function isSipUser(s: string): boolean {
  return s.length > 0 && s.length <= 64 && /^[A-Za-z0-9\-_.!~*'()&=+$,;]+$/.test(s);
}

/**
 * An outbound proxy as `host` or `host:port`, from `host`, `host:port` or a
 * `sip:`/`sips:` URI of either. Null if unusable.
 */
export function normalizeProxy(raw: string): string | null {
  const rest = raw.trim().toLowerCase().replace(/^sips?:/, "");
  const [host, port, ...extra] = rest.split(":");
  if (extra.length > 0 || !isHost(host)) return null;
  if (port === undefined) return host;
  const n = /^\d{1,5}$/.test(port) ? Number(port) : NaN;
  return n >= 1 && n <= 65535 ? `${host}:${n}` : null;
}
