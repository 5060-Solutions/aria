import type { SrtpMode, TransportType } from "../types/sip";

/**
 * What the call screen may say about a call's media.
 *
 * - `encrypted`: SRTP is on and its key went over TLS signalling.
 * - `keysInClear`: SRTP is on, but SDES put its key in the SDP of a UDP or TCP
 *   message, where anyone on the path can read it and decrypt the audio.
 * - `none`: no SRTP.
 *
 * The padlock used to read SRTP alone and showed "Encrypted" for the second
 * case too.
 */
export type MediaSecurity = "encrypted" | "keysInClear" | "none";

export function mediaSecurity(srtpActive: boolean, signalingTls: boolean | undefined): MediaSecurity {
  if (!srtpActive) return "none";
  // A backend that does not report the transport earns no padlock.
  return signalingTls === true ? "encrypted" : "keysInClear";
}

/**
 * Whether an account's settings would send SDES keys in cleartext signalling.
 * `auto` counts: DNS is unsigned, and an attacker who strips the `_sips` SRV
 * record makes it fall back to TCP or UDP.
 */
export function sdesKeysExposed(srtpMode: SrtpMode | undefined, transport: TransportType): boolean {
  return srtpMode === "sdes" && transport !== "tls";
}
