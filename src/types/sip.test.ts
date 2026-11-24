import { describe, expect, it } from "vitest";
import { CODEC_INFO, DEFAULT_CODECS, normalizeCodecs } from "./sip";

describe("codecs", () => {
  it("carries Opus, G.711 and G.729, with G.729 last by default", () => {
    expect(DEFAULT_CODECS.map((c) => c.codec)).toEqual(["opus", "pcmu", "pcma", "g729"]);
    expect(Object.keys(CODEC_INFO).sort()).toEqual(["g729", "opus", "pcma", "pcmu"]);
  });

  it("keeps a saved order and appends the codecs the save predates", () => {
    const saved = [
      { codec: "opus", enabled: true, priority: 1 },
      { codec: "pcmu", enabled: false, priority: 3 },
      { codec: "pcma", enabled: true, priority: 4 },
    ];
    expect(normalizeCodecs(saved)).toEqual([
      { codec: "opus", enabled: true, priority: 1 },
      { codec: "pcmu", enabled: false, priority: 2 },
      { codec: "pcma", enabled: true, priority: 3 },
      { codec: "g729", enabled: true, priority: 4 },
    ]);
  });

  it("drops a saved codec the softphone does not carry", () => {
    const saved = [
      { codec: "ilbc", enabled: true, priority: 1 },
      { codec: "g729", enabled: true, priority: 2 },
      { codec: "opus", enabled: true, priority: 3 },
    ];
    expect(normalizeCodecs(saved)).toEqual([
      { codec: "g729", enabled: true, priority: 1 },
      { codec: "opus", enabled: true, priority: 2 },
      { codec: "pcmu", enabled: true, priority: 3 },
      { codec: "pcma", enabled: true, priority: 4 },
    ]);
  });

  it("falls back to the defaults when nothing saved is usable", () => {
    expect(normalizeCodecs(undefined)).toEqual(DEFAULT_CODECS);
    expect(normalizeCodecs([{ codec: "ilbc", enabled: true, priority: 1 }])).toEqual(DEFAULT_CODECS);
  });
});
