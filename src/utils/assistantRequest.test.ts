import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import fr from "../locales/fr.json";
import de from "../locales/de.json";
import es from "../locales/es.json";
import { REQUEST_TEXT, requestText } from "./assistantRequest";

function lookup(locale: Record<string, unknown>, key: string): string | undefined {
  const [section, name] = key.split(".");
  return (locale[section] as Record<string, string> | undefined)?.[name];
}

describe("assistant request dialog text", () => {
  it.each(Object.entries({ en, fr, de, es }))(
    "%s says what each kind of request would do",
    (_name, locale) => {
      for (const [kind, keys] of Object.entries(REQUEST_TEXT)) {
        expect(lookup(locale, keys.title), `${kind} title`).toBeTruthy();
        const body = lookup(locale, keys.body);
        expect(body, `${kind} body`).toBeTruthy();
        // The body names the party or target, so the user knows what they allow.
        if (kind === "place") {
          expect(body).toContain("{{number}}");
        } else {
          expect(body).toContain("{{remote}}");
        }
        if (kind === "transfer" || kind === "dtmf") {
          expect(body).toContain("{{number}}");
        }
      }
    },
  );

  it("shows an unknown kind as a new call", () => {
    expect(requestText("something-new")).toEqual(REQUEST_TEXT.place);
    expect(requestText("answer")).toEqual(REQUEST_TEXT.answer);
  });
});
