import type { Contact } from "../types/sip";
import type { Translate } from "./connectionReport";

/** One phone number of one person, as `fetch_system_contacts` returns it. */
export interface SystemContact {
  /** Address-book identifier plus the number's position: "ABC:0", "ABC:1". */
  id: string;
  name: string;
  phone: string | null;
  /** The number's label as the address book shows it ("mobile", "work"). */
  label: string | null;
}

/** What `fetch_system_contacts` rejects with. */
export interface ContactsError {
  kind: "denied" | "restricted" | "unsupported" | "failed";
  message: string;
}

/**
 * The digits of a number, keeping only a leading `+`: the dial rules treat
 * `+` as "already international" (see `formatDialNumber`), so it must survive
 * the import while spaces, dashes and parentheses do not.
 */
function dialDigits(phone: string): string {
  const plus = phone.trim().startsWith("+") ? "+" : "";
  return plus + phone.replace(/\D/g, "");
}

function personOf(id: string): string {
  const at = id.lastIndexOf(":");
  return at === -1 ? id : id.slice(0, at);
}

/**
 * Address-book entries as Aria contacts. A person with several numbers is
 * several contacts, told apart by the number's label in the name.
 */
export function mapSystemContacts(entries: SystemContact[]): Contact[] {
  const usable = entries.filter((e): e is SystemContact & { phone: string } => !!e.phone && /\d/.test(e.phone));
  const perPerson = new Map<string, number>();
  for (const e of usable) {
    const person = personOf(e.id);
    perPerson.set(person, (perPerson.get(person) ?? 0) + 1);
  }
  return usable.map((e) => {
    const several = (perPerson.get(personOf(e.id)) ?? 0) > 1;
    const name = several && e.label ? `${e.name} (${e.label})` : e.name;
    return {
      id: `system-${e.id}`,
      name,
      uri: `sip:${dialDigits(e.phone)}`,
      phone: e.phone,
      favorite: false,
      source: "system" as const,
      sourceId: e.id,
    };
  });
}

function isContactsError(e: unknown): e is ContactsError {
  return (
    typeof e === "object" &&
    e !== null &&
    "kind" in e &&
    typeof (e as { kind: unknown }).kind === "string" &&
    "message" in e
  );
}

/**
 * The sentence to show when a sync fails, and whether the cure is the
 * System Settings privacy pane (a refused prompt is never shown again).
 */
export function describeContactsError(e: unknown, t: Translate): { message: string; denied: boolean } {
  if (isContactsError(e)) {
    switch (e.kind) {
      case "denied":
        return { message: t("settings.contactsDenied"), denied: true };
      case "restricted":
        return { message: t("settings.contactsRestricted"), denied: false };
      case "unsupported":
        return { message: t("settings.contactsUnsupported"), denied: false };
      default:
        return { message: t("settings.contactsFailed", { error: e.message }), denied: false };
    }
  }
  const text = e instanceof Error ? e.message : String(e);
  return { message: t("settings.contactsFailed", { error: text }), denied: false };
}
