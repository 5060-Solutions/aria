import type { SipAccount } from "../types/sip";

/**
 * The setup-link fields an edited account keeps. Neither is on the Settings
 * form, so they are carried over from the account being edited:
 * - both are dropped when the username or domain changes, so a setup-link
 *   host cannot later update a line the user has repurposed;
 * - the TLS name goes with the proxy it was set for, so it is kept only while
 *   that proxy is.
 */
export function keptProvisioning(
  editing: SipAccount | null | undefined,
  next: { username: string; domain: string; outboundProxy?: string },
): Pick<SipAccount, "tlsServerName" | "provisionedFrom"> {
  if (!editing) return { tlsServerName: undefined, provisionedFrom: undefined };
  const sameLine =
    editing.username === next.username && editing.domain.trim().toLowerCase() === next.domain.trim().toLowerCase();
  if (!sameLine) return { tlsServerName: undefined, provisionedFrom: undefined };
  const sameProxy = (editing.outboundProxy ?? "") === (next.outboundProxy ?? "");
  return {
    tlsServerName: sameProxy ? editing.tlsServerName : undefined,
    provisionedFrom: editing.provisionedFrom,
  };
}
