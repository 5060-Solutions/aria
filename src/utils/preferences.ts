/** The localStorage key for the user's dark-mode choice. */
export const DARK_MODE_KEY = "aria_dark_mode";

/**
 * Dark mode at launch: the user's saved choice, else the system's. It used to
 * come from the system every time, so a dark-mode choice was lost on restart.
 */
export function initialDarkMode(stored: string | null, systemPrefersDark: boolean): boolean {
  if (stored === "true") return true;
  if (stored === "false") return false;
  return systemPrefersDark;
}

/**
 * The status-bar voicemail button: shown whenever the account has a voicemail
 * number (every Aria PBX setup link sends one) or messages are waiting, so
 * saved messages are reachable without a new one arriving first. It used to
 * appear only while messages were waiting.
 */
export function voicemailButton(
  account: { voicemailNumber?: string } | undefined,
  status: { hasMessages: boolean; newMessages: number } | undefined,
): { visible: boolean; number: string; waiting: boolean } {
  const configured = account?.voicemailNumber?.trim();
  const waiting = !!status?.hasMessages;
  return {
    visible: !!account && (!!configured || waiting),
    // *97 is the Aria PBX's fixed code and a common one elsewhere.
    number: configured || "*97",
    waiting,
  };
}
