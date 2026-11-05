import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import fr from "../locales/fr.json";
import de from "../locales/de.json";
import es from "../locales/es.json";

// The in-call screen's buttons and security pill; a missing key falls back to
// English mid-call.
const callScreenKeys = [
  "mute", "unmute", "keypad", "audio", "hold", "resume", "record", "stopRec",
  "addCall", "transfer", "merge", "swap", "hangUp",
  "encrypted", "notEncrypted", "encryptedTooltip", "notEncryptedRefused", "notEncryptedOff",
] as const;

describe("call screen strings", () => {
  it.each(Object.entries({ en, fr, de, es }))("%s has every call screen string", (_name, locale) => {
    const call = locale.call as Record<string, string>;
    for (const key of callScreenKeys) {
      expect(call[key], key).toBeTruthy();
    }
  });
});
