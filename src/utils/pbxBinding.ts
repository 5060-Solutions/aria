import { invoke } from "@tauri-apps/api/core";
import type { Mailbox } from "./voicemail";

const bound = new Map<string, Promise<void>>();

/**
 * Tell the backend which PBX API an account uses before anything signs in.
 * The backend keeps the address and ignores the one in later commands, so the
 * account's password only goes to the address saved here (or by a setup link);
 * moving an existing account to another address asks the user natively. A
 * refusal rejects, and is asked again next time.
 */
export function ensureBound(mailbox: Mailbox): Promise<void> {
  const key = JSON.stringify(mailbox);
  let p = bound.get(key);
  if (!p) {
    p = invoke("pbx_bind", { mailbox }).then(() => undefined);
    bound.set(key, p);
    p.catch(() => bound.delete(key));
  }
  return p;
}
