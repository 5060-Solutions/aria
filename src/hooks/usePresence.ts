import { useAppStore } from "../stores/appStore";
import type { PresenceState } from "../types/sip";

/**
 * Get the presence state for a given extension or SIP URI.
 * Returns the PresenceState or "offline" if unknown.
 */
export function usePresence(extensionOrUri: string): PresenceState {
  const presenceMap = useAppStore((s) => s.presenceMap);

  // Extract extension from URI if needed (e.g., "sip:1001@domain.com" -> "1001")
  const extension = extractExtension(extensionOrUri);

  return presenceMap[extension] ?? "offline";
}

/**
 * Get the full presence map (all extensions).
 */
export function usePresenceMap(): Record<string, PresenceState> {
  return useAppStore((s) => s.presenceMap);
}

/** Extract the user/extension part from a SIP URI or return as-is */
function extractExtension(uriOrExt: string): string {
  const withoutScheme = uriOrExt.replace(/^sips?:/, "");
  const userPart = withoutScheme.split("@")[0];
  return userPart || withoutScheme;
}
