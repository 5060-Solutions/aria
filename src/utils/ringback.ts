import type { ActiveCall } from "../types/sip";

/**
 * Determine whether local ringback tone should be generated for a call.
 *
 * Per RFC 3960 §3.1 & §3.2, local ringback tone is only played when an outbound
 * call is dialing or ringing AND there is NO in-band early media stream (180/183
 * with SDP) received from the carrier. When in-band early media is present,
 * local tone generation must be suppressed to avoid playing a double ringback tone.
 */
export function shouldPlayRingback(
  call: Pick<ActiveCall, "state" | "hasEarlyMedia"> | null | undefined,
): boolean {
  if (!call) return false;
  return (call.state === "ringing" || call.state === "dialing") && !call.hasEarlyMedia;
}
