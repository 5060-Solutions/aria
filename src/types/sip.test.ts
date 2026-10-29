import { describe, expect, it } from "vitest";
import { CODEC_INFO, DEFAULT_CODECS, normalizeCodecs } from "./sip";

describe("codecs", () => {
  it("offers only the codecs the softphone carries, with no G.729", () => {
    expect(DEFAULT_CODECS.map((c) => c.codec)).toEqual(["opus", "pcmu", "pcma"]);
    expect(Object.keys(CODEC_INFO).sort()).toEqual(["opus", "pcma", "pcmu"]);
  });

  it("loads a saved preference that still names G.729 by dropping it", () => {
    const saved = [
      { codec: "opus", enabled: true, priority: 1 },
      { codec: "g729", enabled: true, priority: 2 },
      { codec: "pcmu", enabled: false, priority: 3 },
      { codec: "pcma", enabled: true, priority: 4 },
    ];
    expect(normalizeCodecs(saved)).toEqual([
      { codec: "opus", enabled: true, priority: 1 },
      { codec: "pcmu", enabled: false, priority: 2 },
      { codec: "pcma", enabled: true, priority: 3 },
    ]);
  });

  it("falls back to the defaults when nothing saved is usable", () => {
    expect(normalizeCodecs(undefined)).toEqual(DEFAULT_CODECS);
    expect(normalizeCodecs([{ codec: "g729", enabled: true, priority: 1 }])).toEqual(DEFAULT_CODECS);
  });
});
