/**
 * Which contacts get a presence (BLF) subscription, and what changes when the
 * contact list does.
 */

/** The internal extension a contact URI names, or null: 2-6 digits only. */
export function blfExtension(uri: string): string | null {
  const user = uri.replace(/^sips?:/, "").split("@")[0];
  return /^\d{2,6}$/.test(user) ? user : null;
}

/** The distinct extensions to watch for a contact list. */
export function blfExtensions(contacts: ReadonlyArray<{ uri: string }>): string[] {
  const out = new Set<string>();
  for (const c of contacts) {
    const ext = blfExtension(c.uri);
    if (ext) out.add(ext);
  }
  return [...out];
}

/**
 * The subscriptions to add and drop when the contact list goes from `before`
 * to `after`. An extension shared by two contacts stays until neither has it.
 */
export function blfDiff(
  before: ReadonlyArray<{ uri: string }>,
  after: ReadonlyArray<{ uri: string }>,
): { add: string[]; remove: string[] } {
  const was = new Set(blfExtensions(before));
  const now = new Set(blfExtensions(after));
  return {
    add: [...now].filter((e) => !was.has(e)),
    remove: [...was].filter((e) => !now.has(e)),
  };
}

/** The backend's answer when no account is registered to subscribe from. */
export function isNotRegistered(e: unknown): boolean {
  return String(e).includes("Not registered");
}
