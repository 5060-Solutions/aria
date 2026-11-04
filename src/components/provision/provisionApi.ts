import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../../stores/appStore";
import { registerAccounts } from "../../hooks/useSip";
import { createProvisionFlow, provisionedAccount } from "../../stores/provisionFlow";
import type { ProvisionApi, ProvisionedLine, ProvisionPreview } from "../../stores/provisionFlow";
import { log } from "../../utils/log";

/** A little longer than the backend's own registration timeout. */
const REGISTER_WAIT_MS = 35000;

/** Resolve with the account's registration error, or null once it registers. */
function waitForRegistration(accountId: string): Promise<string | null> {
  return new Promise((resolve) => {
    let sawRegistering = false;
    let finished = false;
    const finish = (result: string | null) => {
      if (finished) return;
      finished = true;
      unsubscribe();
      clearTimeout(timer);
      resolve(result);
    };
    const unsubscribe = useAppStore.subscribe((state) => {
      const st = state.accountStates[accountId];
      if (!st) return;
      switch (st.registrationState) {
        case "registering":
        case "reconnecting":
          sawRegistering = true;
          break;
        case "registered":
          finish(null);
          break;
        case "error":
          finish(st.registrationError || "Registration failed.");
          break;
        case "unregistered":
          if (sawRegistering) finish("Couldn't connect to the phone server.");
          break;
      }
    });
    const timer = setTimeout(() => finish("The phone server didn't answer in time."), REGISTER_WAIT_MS);
  });
}

/**
 * Save a redeemed line as an account and register it. Which account was
 * decided by the backend: an update only ever targets one this same host set
 * up, and keeps the user's own settings (see `provisionedAccount`).
 */
async function activate(line: ProvisionedLine): Promise<string | null> {
  const store = useAppStore.getState();
  const existing =
    line.mode === "updated" ? store.accounts.find((a) => a.id === line.accountId) : undefined;
  const account = provisionedAccount(line, existing);
  if (existing) {
    store.updateAccount(account);
  } else {
    store.addAccount(account);
  }
  log.info("[Provision] Saved line", { id: account.id, username: account.username, mode: line.mode });

  let error: string | null;
  if (account.enabled) {
    const result = waitForRegistration(account.id);
    await registerAccounts([account]);
    error = await result;
  } else {
    error = "The line is turned off in Settings.";
  }

  // First run: the line is saved whether or not it registered (the link is
  // used up), so leave the setup wizard either way.
  if (!useAppStore.getState().setupComplete) {
    useAppStore.getState().setSetupComplete(true);
  }
  return error;
}

const tauriProvisionApi: ProvisionApi = {
  preview: (url) => invoke<ProvisionPreview>("provision_link_preview", { url }),
  redeem: (url, preview) =>
    invoke<ProvisionedLine>("provision_link_redeem", {
      url,
      expected: { extension: preview.extension, domain: preview.domain },
      accounts: useAppStore
        .getState()
        .accounts.map(({ id, username, domain, provisionedFrom }) => ({
          id,
          username,
          domain,
          provisionedFrom: provisionedFrom ?? null,
        })),
      newAccountId: crypto.randomUUID(),
    }),
  activate,
};

/** The app's one setup-link flow, shared by deep links and the QR scanner. */
export const provisionFlow = createProvisionFlow(tauriProvisionApi);
