import { useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../stores/appStore";
import { log } from "../utils/log";
import { ensureBound } from "../utils/pbxBinding";
import { mailboxFor, type Mailbox } from "../utils/voicemail";

/**
 * Asks the PBX which country the active account's company calls from
 * (`/me`'s `calling_country`, else the admin-only `calling/settings`) and makes it the default country for formatting
 * numbers, unless the person picked one themselves. A seat the PBX will not
 * show the settings to, or an account with no API address, keeps the device's
 * region.
 */
export function usePbxCountry(): void {
  const account = useAppStore((s) => s.accounts.find((a) => a.id === s.activeAccountId));
  const setPbxCountry = useAppStore((s) => s.setPbxCountry);
  // By value: the account object is replaced whenever its registration changes.
  const mailboxJson = JSON.stringify(mailboxFor(account));
  const mailbox = useMemo<Mailbox | null>(() => JSON.parse(mailboxJson) as Mailbox | null, [mailboxJson]);

  useEffect(() => {
    if (!mailbox) return;
    let cancelled = false;
    ensureBound(mailbox)
      .then(() => invoke<string | null>("pbx_calling_country", { mailbox }))
      .then((country) => {
        if (!cancelled && country) setPbxCountry(country);
      })
      .catch((e) => log.info("[calling] company country not available, using the device region:", e));
    return () => {
      cancelled = true;
    };
  }, [mailbox, setPbxCountry]);
}
