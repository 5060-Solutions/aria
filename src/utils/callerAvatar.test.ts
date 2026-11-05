import { describe, expect, it } from "vitest";
import { callerInitials } from "./callerAvatar";

describe("callerInitials", () => {
  it("takes the first and last initials of a name", () => {
    expect(callerInitials("Jane Doe")).toBe("JD");
    expect(callerInitials("Mary Ann van der Berg")).toBe("MB");
  });

  it("uses a single initial for a one-word name", () => {
    expect(callerInitials("Reception")).toBe("R");
  });

  it("handles accented and non-Latin names", () => {
    expect(callerInitials("élodie martin")).toBe("ÉM");
    expect(callerInitials("Ольга Петрова")).toBe("ОП");
  });

  it("returns null for feature codes and numbers", () => {
    expect(callerInitials("*97")).toBeNull();
    expect(callerInitials("+1 (555) 123-4567")).toBeNull();
    expect(callerInitials("1001")).toBeNull();
  });

  it("returns null for addresses", () => {
    expect(callerInitials("sip:alice@example.com")).toBeNull();
    expect(callerInitials("alice@example.com")).toBeNull();
  });

  it("returns null when there is no name", () => {
    expect(callerInitials(undefined)).toBeNull();
    expect(callerInitials(null)).toBeNull();
    expect(callerInitials("   ")).toBeNull();
  });
});
