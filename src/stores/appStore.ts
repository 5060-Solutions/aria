import { create } from "zustand";
import type {
  SipAccount,
  StoredAccount,
  AccountState,
  ActiveCall,
  CallHistoryEntry,
  Contact,
  ContactSource,
  RegistrationState,
  CallState,
  AudioDevice,
  Conference,
} from "../types/sip";
import { storeCredential, getCredential, deleteCredential } from "../utils/credentials";
import { log } from "../utils/log";

type View = "dialer" | "history" | "contacts" | "settings" | "diagnostics";

interface AppState {
  // Theme
  darkMode: boolean;
  toggleDarkMode: () => void;

  // Onboarding
  setupComplete: boolean;
  setSetupComplete: (complete: boolean) => void;

  // SIP Accounts (multi-account support)
  accounts: SipAccount[];
  activeAccountId: string | null;
  accountStates: Record<string, AccountState>;

  // Account management
  addAccount: (account: SipAccount) => void;
  updateAccount: (account: SipAccount) => void;
  removeAccount: (accountId: string) => void;
  setActiveAccount: (accountId: string | null) => void;
  setAccountRegistrationState: (
    accountId: string,
    state: RegistrationState,
    error?: string | null
  ) => void;

  // Convenience getters for active account (backward compatibility)
  account: SipAccount | null;
  registrationState: RegistrationState;
  registrationError: string | null;

  // Legacy methods (operate on active account)
  setAccount: (account: SipAccount) => void;
  setRegistrationState: (state: RegistrationState, error?: string | null) => void;

  // Active Calls (multi-call support)
  activeCalls: ActiveCall[];
  conferences: Conference[];
  
  // Primary call is the one currently in focus (not held, or most recent)
  primaryCallId: string | null;
  
  // Call management
  addCall: (call: ActiveCall) => void;
  updateCall: (callId: string, updates: Partial<ActiveCall>) => void;
  removeCall: (callId: string) => void;
  setPrimaryCall: (callId: string | null) => void;
  
  // Legacy single-call interface (operates on primary call)
  activeCall: ActiveCall | null;
  setActiveCall: (call: ActiveCall | null) => void;
  updateCallState: (state: CallState) => void;
  toggleMute: () => void;
  toggleHold: () => void;
  toggleRecording: () => void;
  
  // Conference management
  createConference: (callIds: string[]) => string;
  addToConference: (conferenceId: string, callId: string) => void;
  removeFromConference: (conferenceId: string, callId: string) => void;
  endConference: (conferenceId: string) => void;

  // Dial pad
  dialInput: string;
  setDialInput: (input: string) => void;
  appendDigit: (digit: string) => void;
  clearDialInput: () => void;

  // Phone number settings
  defaultCountry: string;
  setDefaultCountry: (country: string) => void;

  // Call History
  callHistory: CallHistoryEntry[];
  addCallHistory: (entry: CallHistoryEntry) => void;

  // Contacts
  contacts: Contact[];
  addContact: (contact: Contact) => void;
  updateContact: (contact: Contact) => void;
  removeContact: (id: string) => void;
  importContacts: (contacts: Contact[], source: ContactSource) => void;
  removeContactsBySource: (source: ContactSource) => void;

  // Contacts sync state
  googleConnected: boolean;
  googleLastSync: number | null;
  systemContactsEnabled: boolean;
  systemLastSync: number | null;
  setGoogleConnected: (connected: boolean) => void;
  setGoogleLastSync: (time: number | null) => void;
  setSystemContactsEnabled: (enabled: boolean) => void;
  setSystemLastSync: (time: number | null) => void;

  // Audio
  audioDevices: AudioDevice[];
  selectedInputDevice: string | null;
  selectedOutputDevice: string | null;
  setAudioDevices: (devices: AudioDevice[]) => void;
  setSelectedInputDevice: (id: string) => void;
  setSelectedOutputDevice: (id: string) => void;

  // Recordings
  recordingsDirectory: string | null;
  setRecordingsDirectory: (path: string | null) => void;

  // Navigation
  currentView: View;
  setCurrentView: (view: View) => void;
}

export const useAppStore = create<AppState>((set, get) => {
  const initialData = loadAccounts();

  return {
    // Theme
    darkMode: window.matchMedia("(prefers-color-scheme: dark)").matches,
    toggleDarkMode: () => set((s) => ({ darkMode: !s.darkMode })),

    // Onboarding
    setupComplete: localStorage.getItem("aria_setup_complete") === "true",
    setSetupComplete: (complete) => {
      localStorage.setItem("aria_setup_complete", String(complete));
      set({ setupComplete: complete });
    },

    // SIP Accounts (multi-account)
    accounts: initialData.accounts,
    activeAccountId: initialData.activeAccountId,
    accountStates: initialData.accountStates,

    // Account management
    addAccount: (account) =>
      set((s) => {
        const accounts = [...s.accounts, account];
        // Preserve existing registration state if it exists (e.g., from backend event)
        const existingState = s.accountStates[account.id];
        const accountStates = {
          ...s.accountStates,
          [account.id]: existingState ?? {
            accountId: account.id,
            registrationState: "unregistered" as RegistrationState,
            registrationError: null,
          },
        };
        const activeAccountId = s.activeAccountId ?? account.id;
        saveAccounts(accounts, activeAccountId);
        return { accounts, accountStates, activeAccountId };
      }),

    updateAccount: (account) =>
      set((s) => {
        const accounts = s.accounts.map((a) =>
          a.id === account.id ? account : a
        );
        saveAccounts(accounts, s.activeAccountId);
        return { accounts };
      }),

    removeAccount: (accountId) =>
      set((s) => {
        const accounts = s.accounts.filter((a) => a.id !== accountId);
        const accountStates = Object.fromEntries(
          Object.entries(s.accountStates).filter(([id]) => id !== accountId)
        ) as Record<string, AccountState>;
        const activeAccountId =
          s.activeAccountId === accountId
            ? accounts[0]?.id ?? null
            : s.activeAccountId;
        saveAccounts(accounts, activeAccountId);
        return { accounts, accountStates, activeAccountId };
      }),

    setActiveAccount: (accountId) =>
      set((s) => {
        saveAccounts(s.accounts, accountId);
        return { activeAccountId: accountId };
      }),

    setAccountRegistrationState: (accountId, state, error) =>
      set((s) => ({
        accountStates: {
          ...s.accountStates,
          [accountId]: {
            accountId,
            registrationState: state,
            registrationError: error ?? null,
          },
        },
      })),

    // Convenience getters (computed from active account)
    get account() {
      const state = get();
      return state.accounts.find((a) => a.id === state.activeAccountId) ?? null;
    },
    get registrationState() {
      const state = get();
      if (!state.activeAccountId) return "unregistered";
      return (
        state.accountStates[state.activeAccountId]?.registrationState ??
        "unregistered"
      );
    },
    get registrationError() {
      const state = get();
      if (!state.activeAccountId) return null;
      return state.accountStates[state.activeAccountId]?.registrationError ?? null;
    },

    // Legacy methods (operate on active account for backward compatibility)
    setAccount: (account) => {
      const state = get();
      const existing = state.accounts.find((a) => a.id === account.id);
      if (existing) {
        state.updateAccount(account);
      } else {
        state.addAccount(account);
      }
      state.setActiveAccount(account.id);
    },

    setRegistrationState: (regState, error) => {
      const state = get();
      if (state.activeAccountId) {
        state.setAccountRegistrationState(state.activeAccountId, regState, error);
      }
    },

    // Active Calls (multi-call support)
    activeCalls: [],
    conferences: [],
    primaryCallId: null,
    activeCall: null, // Derived from activeCalls + primaryCallId, kept in sync
    
    // Call management
    addCall: (call) =>
      set((s) => {
        const activeCalls = [...s.activeCalls, call];
        const primaryCallId = s.primaryCallId ?? call.id;
        return {
          activeCalls,
          primaryCallId,
          activeCall: activeCalls.find((c) => c.id === primaryCallId) ?? null,
        };
      }),
    
    updateCall: (callId, updates) =>
      set((s) => {
        const activeCalls = s.activeCalls.map((c) =>
          c.id === callId ? { ...c, ...updates } : c
        );
        return {
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === s.primaryCallId) ?? null,
        };
      }),
    
    removeCall: (callId) =>
      set((s) => {
        const activeCalls = s.activeCalls.filter((c) => c.id !== callId);
        let primaryCallId = s.primaryCallId;
        if (primaryCallId === callId) {
          primaryCallId = activeCalls.find((c) => c.state !== "ended")?.id ?? null;
        }
        return {
          activeCalls,
          primaryCallId,
          activeCall: activeCalls.find((c) => c.id === primaryCallId) ?? null,
        };
      }),
    
    setPrimaryCall: (callId) =>
      set((s) => ({
        primaryCallId: callId,
        activeCall: s.activeCalls.find((c) => c.id === callId) ?? null,
      })),
    
    // Legacy single-call interface (operates on primary call)
    setActiveCall: (call) =>
      set((s) => {
        if (call === null) {
          return { activeCalls: [], primaryCallId: null, conferences: [], activeCall: null };
        }
        
        // Check if there's an existing call with the same ID
        const existingById = s.activeCalls.find((c) => c.id === call.id);
        if (existingById) {
          const activeCalls = s.activeCalls.map((c) => (c.id === call.id ? call : c));
          return {
            activeCalls,
            primaryCallId: call.id,
            activeCall: call,
          };
        }
        
        // Check if there's a pending outbound call to the same URI that should be replaced
        // (handles the case where dialer creates a temp call before getting real call ID)
        const pendingOutbound = s.activeCalls.find(
          (c) => c.remoteUri === call.remoteUri && 
                 c.direction === "outbound" && 
                 c.state === "dialing"
        );
        if (pendingOutbound) {
          const activeCalls = s.activeCalls.map((c) => 
            c.id === pendingOutbound.id ? call : c
          );
          return {
            activeCalls,
            primaryCallId: call.id,
            activeCall: call,
          };
        }
        
        // Add new call
        const activeCalls = [...s.activeCalls, call];
        return {
          activeCalls,
          primaryCallId: call.id,
          activeCall: call,
        };
      }),
    
    updateCallState: (state) =>
      set((s) => {
        const callId = s.primaryCallId;
        if (!callId) return s;
        const activeCalls = s.activeCalls.map((c) =>
          c.id === callId ? { ...c, state } : c
        );
        return {
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === callId) ?? null,
        };
      }),
    
    toggleMute: () =>
      set((s) => {
        const callId = s.primaryCallId;
        if (!callId) return s;
        const activeCalls = s.activeCalls.map((c) =>
          c.id === callId ? { ...c, muted: !c.muted } : c
        );
        return {
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === callId) ?? null,
        };
      }),
    
    toggleHold: () =>
      set((s) => {
        const callId = s.primaryCallId;
        if (!callId) return s;
        const activeCalls = s.activeCalls.map((c) =>
          c.id === callId ? { ...c, held: !c.held } : c
        );
        return {
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === callId) ?? null,
        };
      }),
    
    toggleRecording: () =>
      set((s) => {
        const callId = s.primaryCallId;
        if (!callId) return s;
        const activeCalls = s.activeCalls.map((c) =>
          c.id === callId ? { ...c, recording: !c.recording } : c
        );
        return {
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === callId) ?? null,
        };
      }),
    
    // Conference management
    createConference: (callIds) => {
      const conferenceId = `conf-${Date.now()}`;
      set((s) => {
        const activeCalls = s.activeCalls.map((c) =>
          callIds.includes(c.id)
            ? { ...c, conferenceId, state: "conferenced" as CallState, held: false }
            : c
        );
        return {
          conferences: [
            ...s.conferences,
            { id: conferenceId, callIds, startTime: Date.now() },
          ],
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === s.primaryCallId) ?? null,
        };
      });
      return conferenceId;
    },
    
    addToConference: (conferenceId, callId) =>
      set((s) => {
        const activeCalls = s.activeCalls.map((c) =>
          c.id === callId
            ? { ...c, conferenceId, state: "conferenced" as CallState, held: false }
            : c
        );
        return {
          conferences: s.conferences.map((conf) =>
            conf.id === conferenceId
              ? { ...conf, callIds: [...conf.callIds, callId] }
              : conf
          ),
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === s.primaryCallId) ?? null,
        };
      }),
    
    removeFromConference: (conferenceId, callId) =>
      set((s) => {
        const activeCalls = s.activeCalls.map((c) =>
          c.id === callId
            ? { ...c, conferenceId: undefined, state: "connected" as CallState }
            : c
        );
        return {
          conferences: s.conferences.map((conf) =>
            conf.id === conferenceId
              ? { ...conf, callIds: conf.callIds.filter((id) => id !== callId) }
              : conf
          ),
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === s.primaryCallId) ?? null,
        };
      }),
    
    endConference: (conferenceId) =>
      set((s) => {
        const activeCalls = s.activeCalls.map((c) =>
          c.conferenceId === conferenceId
            ? { ...c, conferenceId: undefined, state: "connected" as CallState }
            : c
        );
        return {
          conferences: s.conferences.filter((conf) => conf.id !== conferenceId),
          activeCalls,
          activeCall: activeCalls.find((c) => c.id === s.primaryCallId) ?? null,
        };
      }),

    // Dial pad
    dialInput: "",
    setDialInput: (dialInput) => set({ dialInput }),
    appendDigit: (digit) => set((s) => ({ dialInput: s.dialInput + digit })),
    clearDialInput: () => set({ dialInput: "" }),

    // Phone number settings
    defaultCountry: localStorage.getItem("aria_default_country") || "US",
    setDefaultCountry: (country) => {
      localStorage.setItem("aria_default_country", country);
      set({ defaultCountry: country });
    },

    // Call History
    callHistory: loadCallHistory(),
    addCallHistory: (entry) =>
      set((s) => {
        const callHistory = [entry, ...s.callHistory].slice(0, 200);
        localStorage.setItem("aria_call_history", JSON.stringify(callHistory));
        return { callHistory };
      }),

    // Contacts
    contacts: loadContacts(),
    addContact: (contact) =>
      set((s) => {
        const contactWithSource = { ...contact, source: contact.source ?? "local" as ContactSource };
        const contacts = [...s.contacts, contactWithSource];
        localStorage.setItem("aria_contacts", JSON.stringify(contacts));
        return { contacts };
      }),
    updateContact: (contact) =>
      set((s) => {
        const contacts = s.contacts.map((c) => (c.id === contact.id ? contact : c));
        localStorage.setItem("aria_contacts", JSON.stringify(contacts));
        return { contacts };
      }),
    removeContact: (id) =>
      set((s) => {
        const contacts = s.contacts.filter((c) => c.id !== id);
        localStorage.setItem("aria_contacts", JSON.stringify(contacts));
        return { contacts };
      }),
    importContacts: (newContacts, source) =>
      set((s) => {
        // Remove existing contacts from this source, then add new ones
        const otherContacts = s.contacts.filter((c) => c.source !== source);
        // Deduplicate by sourceId within the new contacts
        const deduped = newContacts.filter(
          (c, i, arr) => !c.sourceId || arr.findIndex((x) => x.sourceId === c.sourceId) === i
        );
        const contacts = [...otherContacts, ...deduped];
        localStorage.setItem("aria_contacts", JSON.stringify(contacts));
        return { contacts };
      }),
    removeContactsBySource: (source) =>
      set((s) => {
        const contacts = s.contacts.filter((c) => c.source !== source);
        localStorage.setItem("aria_contacts", JSON.stringify(contacts));
        return { contacts };
      }),

    // Contacts sync state
    googleConnected: localStorage.getItem("aria_google_connected") === "true",
    googleLastSync: (() => {
      const ts = localStorage.getItem("aria_google_last_sync");
      return ts ? parseInt(ts, 10) : null;
    })(),
    systemContactsEnabled: localStorage.getItem("aria_system_contacts_enabled") === "true",
    systemLastSync: (() => {
      const ts = localStorage.getItem("aria_system_last_sync");
      return ts ? parseInt(ts, 10) : null;
    })(),
    setGoogleConnected: (connected) => {
      localStorage.setItem("aria_google_connected", String(connected));
      set({ googleConnected: connected });
    },
    setGoogleLastSync: (time) => {
      if (time !== null) {
        localStorage.setItem("aria_google_last_sync", String(time));
      } else {
        localStorage.removeItem("aria_google_last_sync");
      }
      set({ googleLastSync: time });
    },
    setSystemContactsEnabled: (enabled) => {
      localStorage.setItem("aria_system_contacts_enabled", String(enabled));
      set({ systemContactsEnabled: enabled });
    },
    setSystemLastSync: (time) => {
      if (time !== null) {
        localStorage.setItem("aria_system_last_sync", String(time));
      } else {
        localStorage.removeItem("aria_system_last_sync");
      }
      set({ systemLastSync: time });
    },

    // Audio
    audioDevices: [],
    selectedInputDevice: null,
    selectedOutputDevice: null,
    setAudioDevices: (audioDevices) => set({ audioDevices }),
    setSelectedInputDevice: (selectedInputDevice) =>
      set({ selectedInputDevice }),
    setSelectedOutputDevice: (selectedOutputDevice) =>
      set({ selectedOutputDevice }),

    // Recordings
    recordingsDirectory: localStorage.getItem("aria_recordings_dir"),
    setRecordingsDirectory: (path) => {
      if (path) {
        localStorage.setItem("aria_recordings_dir", path);
      } else {
        localStorage.removeItem("aria_recordings_dir");
      }
      set({ recordingsDirectory: path });
    },

    // Navigation
    currentView: "dialer",
    setCurrentView: (currentView) => set({ currentView }),
  };
});

interface StoredAccountData {
  accounts: SipAccount[];
  activeAccountId: string | null;
  accountStates: Record<string, AccountState>;
}

/** Strip password from account for localStorage storage */
function toStoredAccount(account: SipAccount): StoredAccount {
  const copy = { ...account } as unknown as Record<string, unknown>;
  delete copy.password;
  return copy as unknown as StoredAccount;
}

/** Convert stored account to full account (password will be empty, needs to be loaded separately) */
function fromStoredAccount(stored: StoredAccount): SipAccount {
  // Migrate old srtpMode values: "optional" and "required" → "sdes"
  let srtpMode = stored.srtpMode;
  if (srtpMode === ("optional" as string) || srtpMode === ("required" as string)) {
    srtpMode = "sdes";
  }
  return { ...stored, srtpMode, password: "" };
}

function loadAccounts(): StoredAccountData {
  try {
    // Try new multi-account format first (without passwords)
    const stored = localStorage.getItem("aria_accounts_v2");
    if (stored) {
      const data = JSON.parse(stored) as {
        accounts: StoredAccount[];
        activeAccountId: string | null;
      };
      const accountStates: Record<string, AccountState> = {};
      const accounts: SipAccount[] = [];
      for (const acc of data.accounts) {
        accounts.push(fromStoredAccount(acc));
        accountStates[acc.id] = {
          accountId: acc.id,
          registrationState: "unregistered",
          registrationError: null,
        };
      }
      return { accounts, activeAccountId: data.activeAccountId, accountStates };
    }

    // Migrate from old format (with passwords in localStorage - insecure)
    const oldStored = localStorage.getItem("aria_accounts");
    if (oldStored) {
      const data = JSON.parse(oldStored) as {
        accounts: SipAccount[];
        activeAccountId: string | null;
      };
      const accountStates: Record<string, AccountState> = {};
      for (const acc of data.accounts) {
        accountStates[acc.id] = {
          accountId: acc.id,
          registrationState: "unregistered",
          registrationError: null,
        };
        // Migrate password to secure storage (async, fire and forget for migration)
        if (acc.password) {
          storeCredential(acc.id, acc.password).catch(() => {});
        }
      }
      // Save in new format without passwords
      saveAccountsInternal(data.accounts, data.activeAccountId);
      localStorage.removeItem("aria_accounts");
      return { ...data, accountStates };
    }

    // Migrate from legacy single-account format
    const legacyAccount = localStorage.getItem("aria_account");
    if (legacyAccount) {
      const account = JSON.parse(legacyAccount) as SipAccount;
      if (account.enabled === undefined) {
        account.enabled = true;
      }
      const accounts = [account];
      const activeAccountId = account.id;
      const accountStates: Record<string, AccountState> = {
        [account.id]: {
          accountId: account.id,
          registrationState: "unregistered",
          registrationError: null,
        },
      };
      // Migrate password to secure storage
      if (account.password) {
        storeCredential(account.id, account.password).catch(() => {});
      }
      // Save in new format and remove legacy
      saveAccountsInternal(accounts, activeAccountId);
      localStorage.removeItem("aria_account");
      return { accounts, activeAccountId, accountStates };
    }

    return { accounts: [], activeAccountId: null, accountStates: {} };
  } catch {
    return { accounts: [], activeAccountId: null, accountStates: {} };
  }
}

/** Internal save that strips passwords */
function saveAccountsInternal(accounts: SipAccount[], activeAccountId: string | null) {
  const storedAccounts = accounts.map(toStoredAccount);
  localStorage.setItem(
    "aria_accounts_v2",
    JSON.stringify({ accounts: storedAccounts, activeAccountId })
  );
}

function saveAccounts(accounts: SipAccount[], activeAccountId: string | null) {
  saveAccountsInternal(accounts, activeAccountId);
}

/** Load password for an account from secure storage */
export async function loadAccountPassword(accountId: string): Promise<string | null> {
  try {
    const password = await getCredential(accountId);
    log.info(`[loadAccountPassword] accountId=${accountId}, hasPassword=${!!password}, length=${password?.length ?? 0}`);
    return password;
  } catch (e) {
    log.error(`[loadAccountPassword] Failed for accountId=${accountId}:`, e);
    return null;
  }
}

/** Save password for an account to secure storage */
export async function saveAccountPassword(accountId: string, password: string): Promise<void> {
  log.info(`[saveAccountPassword] Saving password for accountId=${accountId}, length=${password.length}`);
  try {
    await storeCredential(accountId, password);
    log.info(`[saveAccountPassword] Successfully saved password for accountId=${accountId}`);
  } catch (e) {
    log.error(`[saveAccountPassword] Failed to save password for accountId=${accountId}:`, e);
    throw e;
  }
}

/** Delete password for an account from secure storage */
export async function deleteAccountPassword(accountId: string): Promise<void> {
  await deleteCredential(accountId);
}

/** Get a full account with password loaded from secure storage */
export async function getAccountWithPassword(account: SipAccount): Promise<SipAccount> {
  const password = await loadAccountPassword(account.id);
  return { ...account, password: password ?? "" };
}

function loadContacts(): Contact[] {
  try {
    const stored = localStorage.getItem("aria_contacts");
    if (!stored) return [];
    const contacts = JSON.parse(stored) as Contact[];
    // Migrate contacts without source field to "local"
    return contacts.map((c) => ({
      ...c,
      source: c.source ?? ("local" as ContactSource),
    }));
  } catch {
    return [];
  }
}

function loadCallHistory(): CallHistoryEntry[] {
  try {
    const stored = localStorage.getItem("aria_call_history");
    return stored ? (JSON.parse(stored) as CallHistoryEntry[]) : [];
  } catch {
    return [];
  }
}
