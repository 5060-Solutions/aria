import { describe, expect, it } from "vitest";
import { describeContactsError, mapSystemContacts, type SystemContact } from "./systemContacts";

function entry(id: string, name: string, phone: string | null, label: string | null = null): SystemContact {
  return { id, name, phone, label };
}

describe("mapSystemContacts", () => {
  it("imports a number as a sip: URI of digits, keeping a leading plus", () => {
    const [c] = mapSystemContacts([entry("ABC:0", "Franco Rossi", "+39 333 123-4567", "mobile")]);
    expect(c).toEqual({
      id: "system-ABC:0",
      name: "Franco Rossi",
      uri: "sip:+393331234567",
      phone: "+39 333 123-4567",
      favorite: false,
      source: "system",
      sourceId: "ABC:0",
    });
  });

  it("drops a plus that is not at the front", () => {
    const [c] = mapSystemContacts([entry("A:0", "X", "0800+1")]);
    expect(c.uri).toBe("sip:08001");
  });

  it("labels the name only when a person has more than one number", () => {
    const got = mapSystemContacts([
      entry("ABC:0", "Franco Rossi", "+39 333 1234567", "mobile"),
      entry("ABC:1", "Franco Rossi", "+39 02 1234567", "work"),
      entry("DEF:0", "Anna", "555 0100", "home"),
      entry("GHI:0", "No Label", "555 0101"),
      entry("GHI:1", "No Label", "555 0102"),
    ]);
    expect(got.map((c) => c.name)).toEqual(["Franco Rossi (mobile)", "Franco Rossi (work)", "Anna", "No Label", "No Label"]);
  });

  it("skips entries without a usable number", () => {
    expect(mapSystemContacts([entry("A:0", "X", null), entry("B:0", "Y", "n/a")])).toEqual([]);
  });
});

describe("describeContactsError", () => {
  const t = (key: string, opts?: Record<string, unknown>) => (opts?.error ? `${key}:${String(opts.error)}` : key);

  it("recognises the backend's structured error", () => {
    expect(describeContactsError({ kind: "denied", message: "no" }, t)).toEqual({
      message: "settings.contactsDenied",
      denied: true,
    });
    expect(describeContactsError({ kind: "restricted", message: "no" }, t)).toEqual({
      message: "settings.contactsRestricted",
      denied: false,
    });
    expect(describeContactsError({ kind: "unsupported", message: "no" }, t)).toEqual({
      message: "settings.contactsUnsupported",
      denied: false,
    });
    expect(describeContactsError({ kind: "failed", message: "boom" }, t)).toEqual({
      message: "settings.contactsFailed:boom",
      denied: false,
    });
  });

  it("falls back to the error's own text for anything else", () => {
    expect(describeContactsError(new Error("bad"), t)).toEqual({ message: "settings.contactsFailed:bad", denied: false });
    expect(describeContactsError("plain", t)).toEqual({ message: "settings.contactsFailed:plain", denied: false });
  });
});
