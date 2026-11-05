import { describe, expect, it } from "vitest";
import { licenseUrl, platformLabel, releaseNotesUrl, versionDetails } from "./about";
import en from "../locales/en.json";
import fr from "../locales/fr.json";
import de from "../locales/de.json";
import es from "../locales/es.json";

describe("releaseNotesUrl", () => {
  it("points at the v-prefixed release tag", () => {
    expect(releaseNotesUrl("0.6.3")).toBe(
      "https://github.com/5060-Solutions/aria/releases/tag/v0.6.3"
    );
  });

  it("does not double the prefix", () => {
    expect(releaseNotesUrl("v0.6.3")).toBe(
      "https://github.com/5060-Solutions/aria/releases/tag/v0.6.3"
    );
  });
});

describe("licenseUrl", () => {
  it("pins the license to the running version's tag", () => {
    expect(licenseUrl("0.6.3")).toBe(
      "https://github.com/5060-Solutions/aria/blob/v0.6.3/LICENSE"
    );
  });

  it("falls back to main before the version is known", () => {
    expect(licenseUrl(null)).toBe("https://github.com/5060-Solutions/aria/blob/main/LICENSE");
  });
});

describe("platformLabel", () => {
  it("names the platform the way people do", () => {
    expect(platformLabel({ os: "macos", arch: "aarch64" })).toBe("macOS · arm64");
    expect(platformLabel({ os: "windows", arch: "x86_64" })).toBe("Windows · x64");
  });

  it("passes unknown names through", () => {
    expect(platformLabel({ os: "freebsd", arch: "riscv64" })).toBe("freebsd · riscv64");
  });
});

describe("versionDetails", () => {
  it("includes the commit and platform when known", () => {
    expect(
      versionDetails("0.6.3", { commit: "4e51409abc", os: "macos", arch: "aarch64", release: true })
    ).toBe("Aria 0.6.3 (4e51409abc) macOS arm64");
  });

  it("marks debug builds", () => {
    expect(
      versionDetails("0.6.3", { commit: null, os: "linux", arch: "x86_64", release: false })
    ).toBe("Aria 0.6.3 Linux x64 debug");
  });

  it("works before the build info has loaded", () => {
    expect(versionDetails("0.6.3", null)).toBe("Aria 0.6.3");
  });
});

describe("about strings", () => {
  const locales = { en, fr, de, es };
  const englishKeys = Object.keys(en.about).sort();

  it.each(Object.entries(locales))("%s has every About string", (_name, locale) => {
    expect(Object.keys(locale.about).sort()).toEqual(englishKeys);
    for (const value of Object.values(locale.about)) {
      expect(typeof value).toBe("string");
      expect(value).not.toBe("");
    }
  });

  it.each(Object.entries({ fr, de, es }))("%s translates the About strings", (_name, locale) => {
    // The title at least must not be the English one left in place.
    expect(locale.about.title).not.toBe(en.about.title);
  });
});
