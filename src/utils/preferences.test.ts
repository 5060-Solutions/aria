import { describe, expect, it } from "vitest";
import { initialDarkMode, voicemailButton } from "./preferences";

describe("initialDarkMode", () => {
  it("prefers the saved choice over the system", () => {
    expect(initialDarkMode("true", false)).toBe(true);
    expect(initialDarkMode("false", true)).toBe(false);
  });

  it("follows the system when nothing is saved", () => {
    expect(initialDarkMode(null, true)).toBe(true);
    expect(initialDarkMode(null, false)).toBe(false);
    expect(initialDarkMode("garbage", true)).toBe(true);
  });
});

describe("voicemailButton", () => {
  it("is always shown for an account with a voicemail number", () => {
    expect(voicemailButton({ voicemailNumber: "*97" }, undefined)).toEqual({
      visible: true,
      number: "*97",
      waiting: false,
    });
    expect(voicemailButton({ voicemailNumber: " 8500 " }, { hasMessages: true, newMessages: 2 })).toEqual({
      visible: true,
      number: "8500",
      waiting: true,
    });
  });

  it("shows for waiting messages without a number, dialling *97", () => {
    expect(voicemailButton({}, { hasMessages: true, newMessages: 1 })).toEqual({
      visible: true,
      number: "*97",
      waiting: true,
    });
  });

  it("is always shown for an Aria PBX account, which opens the mailbox in the app", () => {
    expect(voicemailButton({ pbxApiUrl: "https://api.ariaroute.com" }, undefined)).toEqual({
      visible: true,
      number: "*97",
      waiting: false,
    });
    expect(voicemailButton({ pbxApiUrl: "  " }, undefined).visible).toBe(false);
  });

  it("hides for a hand-made account with nothing waiting, and with no account", () => {
    expect(voicemailButton({}, { hasMessages: false, newMessages: 0 }).visible).toBe(false);
    expect(voicemailButton(undefined, { hasMessages: true, newMessages: 1 }).visible).toBe(false);
  });
});
