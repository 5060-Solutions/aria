import { describe, expect, it } from "vitest";
import en from "./en.json";
import de from "./de.json";
import es from "./es.json";
import fr from "./fr.json";

type Tree = { [key: string]: string | Tree };

function flatten(tree: Tree, prefix = ""): string[] {
  return Object.entries(tree).flatMap(([k, v]) =>
    typeof v === "string" ? [prefix + k] : flatten(v, `${prefix}${k}.`),
  );
}

/** i18next plural forms resolve `key` from `key_one` / `key_other`. */
function withPluralBases(keys: string[]): Set<string> {
  const out = new Set(keys);
  for (const k of keys) {
    const m = /^(.*)_(zero|one|two|few|many|other)$/.exec(k);
    if (m) out.add(m[1]);
  }
  return out;
}

/** Every non-test source file under src/, as text. */
const sources = import.meta.glob(["../**/*.ts", "../**/*.tsx", "!../**/*.test.ts"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const enKeys = flatten(en as Tree);
const enSet = withPluralBases(enKeys);

describe("locales", () => {
  for (const [name, tree] of Object.entries({ de, es, fr })) {
    it(`${name} has every key en has`, () => {
      const have = new Set(flatten(tree as Tree));
      // Plural suffixes differ by language; compare on the base key.
      const missing = enKeys.filter((k) => !have.has(k) && !withPluralBases([...have]).has(k.replace(/_(one|other)$/, "")));
      expect(missing).toEqual([]);
    });
  }

  it("every literal t() key in the source exists in en", () => {
    const used = new Set<string>();
    expect(Object.keys(sources).length).toBeGreaterThan(20);
    for (const text of Object.values(sources)) {
      // t("a.b") and i18n.t("a.b"); skips template-literal keys.
      for (const m of text.matchAll(/\bt\(\s*"([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)+)"/g)) {
        used.add(m[1]);
      }
    }
    const missing = [...used].filter((k) => !enSet.has(k)).sort();
    expect(missing).toEqual([]);
  });
});
