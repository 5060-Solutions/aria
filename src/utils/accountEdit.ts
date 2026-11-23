import type { SipAccount } from "../types/sip";

/**
 * The setup-link fields an edited account keeps. None is on the Settings
 * form, so they are carried over from the account being edited:
 * - the link host and TLS name are dropped when the username or domain
 *   changes, so a setup-link host cannot later update a line the user has
 *   repurposed;
 * - the TLS name goes with the proxy it was set for, so it is kept only while
 *   that proxy is;
 * - the voicemail number is a feature code of the PBX, so it is kept while
 *   the domain is. Saving used to drop it, and the voicemail button then
 *   dialled the *97 fallback instead of the provisioned number.
 */
export function keptProvisioning(
  editing: SipAccount | null | undefined,
  next: { username: string; domain: string; outboundProxy?: string },
): Pick<SipAccount, "tlsServerName" | "provisionedFrom" | "voicemailNumber"> {
  const none = { tlsServerName: undefined, provisionedFrom: undefined, voicemailNumber: undefined };
  if (!editing) return none;
  const sameDomain = editing.domain.trim().toLowerCase() === next.domain.trim().toLowerCase();
  const voicemailNumber = sameDomain ? editing.voicemailNumber : undefined;
  if (!sameDomain || editing.username !== next.username) return { ...none, voicemailNumber };
  const sameProxy = (editing.outboundProxy ?? "") === (next.outboundProxy ?? "");
  return {
    tlsServerName: sameProxy ? editing.tlsServerName : undefined,
    provisionedFrom: editing.provisionedFrom,
    voicemailNumber,
  };
}
