import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../../stores/appStore";
import { registerAccounts } from "../../hooks/useSip";
import { createProvisionFlow } from "../../stores/provisionFlow";
import type { ProvisionApi, ProvisionedLine, ProvisionPreview } from "../../stores/provisionFlow";
import type { SipAccount, TransportType } from "../../types/sip";
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
 * Save a redeemed line as an account — updating the existing account for the
 * same username@domain in place — and register it.
 */
async function activate(line: ProvisionedLine): Promise<string | null> {
  const store = useAppStore.getState();
  const existing = store.accounts.find((a) => a.id === line.accountId);
  const account: SipAccount = {
    // An update keeps the user's own per-account settings (codecs, SRTP,
    // dialing rules); the connection settings come from the link.
    ...existing,
    id: line.accountId,
    displayName: line.displayName || line.username,
    username: line.username,
    domain: line.domain,
    // Already in the credential store; never kept in app state.
    password: "",
    transport: line.transport as TransportType,
    port: line.port,
    registrar: line.registrar ?? undefined,
    outboundProxy: line.outboundProxy ?? undefined,
    authUsername: line.authUsername ?? undefined,
    authRealm: undefined,
    enabled: true,
    voicemailNumber: line.voicemail ?? existing?.voicemailNumber,
  };
  if (existing) {
    store.updateAccount(account);
  } else {
    store.addAccount(account);
  }
  log.info("[Provision] Saved line", { id: account.id, username: account.username, updated: !!existing });

  const result = waitForRegistration(account.id);
  await registerAccounts([account]);
  const error = await result;

  // First run: the line is saved whether or not it registered (the link is
  // used up), so leave the setup wizard either way.
  if (!useAppStore.getState().setupComplete) {
    useAppStore.getState().setSetupComplete(true);
  }
  return error;
}

const tauriProvisionApi: ProvisionApi = {
  preview: (url) => invoke<ProvisionPreview>("provision_link_preview", { url }),
  redeem: (url) =>
    invoke<ProvisionedLine>("provision_link_redeem", {
      url,
      accounts: useAppStore
        .getState()
        .accounts.map(({ id, username, domain }) => ({ id, username, domain })),
      newAccountId: crypto.randomUUID(),
    }),
  activate,
};

/** The app's one setup-link flow, shared by deep links and the QR scanner. */
export const provisionFlow = createProvisionFlow(tauriProvisionApi);
