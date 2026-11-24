import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "./toastStore";
import { log } from "../utils/log";
import { isVoicemailError, type Mailbox, type VoicemailError, type VoicemailMessage } from "../utils/voicemail";

interface MailboxState {
  /** The mailbox these messages came from, so an MWI can refresh it. */
  mailbox: Mailbox;
  messages: VoicemailMessage[];
  loading: boolean;
  /** The last failure; cleared by a refresh that succeeds. */
  error: VoicemailError | null;
  /** Whether a load has ever finished, so an empty list is not a spinner. */
  loaded: boolean;
}

interface VoicemailStoreState {
  byAccount: Record<string, MailboxState>;
  /** Load (or reload) the mailbox. Errors land in the entry, not a toast. */
  refresh: (mailbox: Mailbox) => Promise<void>;
  /** Mark a message read on the PBX and locally. */
  markRead: (mailbox: Mailbox, messageId: string) => Promise<void>;
  /** Delete a message on the PBX and locally; rejects so the UI can say so. */
  remove: (mailbox: Mailbox, messageId: string) => Promise<void>;
  /** The message's WAV bytes. */
  loadAudio: (mailbox: Mailbox, messageId: string) => Promise<ArrayBuffer>;
  /** The PBX said the message count changed: reload if this mailbox is open. */
  onMwi: (accountId: string) => void;
}

function asVoicemailError(e: unknown): VoicemailError {
  return isVoicemailError(e) ? e : { kind: "network", message: errorText(e) };
}

/**
 * Visual voicemail, per account. Only accounts with a PBX API address
 * (`mailboxFor`) ever get an entry; everyone else keeps the dial-in button.
 */
export const useVoicemailStore = create<VoicemailStoreState>((set, get) => ({
  byAccount: {},

  refresh: async (mailbox) => {
    const id = mailbox.accountId;
    set((s) => ({
      byAccount: {
        ...s.byAccount,
        [id]: { ...(s.byAccount[id] ?? { messages: [], error: null, loaded: false }), mailbox, loading: true },
      },
    }));
    try {
      const messages = await invoke<VoicemailMessage[]>("voicemail_list", { mailbox });
      set((s) => ({
        byAccount: { ...s.byAccount, [id]: { mailbox, messages, loading: false, error: null, loaded: true } },
      }));
    } catch (e) {
      const error = asVoicemailError(e);
      log.warn("[voicemail] load failed:", error.kind, error.message);
      set((s) => ({
        byAccount: {
          ...s.byAccount,
          [id]: { ...(s.byAccount[id] ?? { messages: [] }), mailbox, loading: false, error, loaded: true },
        },
      }));
    }
  },

  markRead: async (mailbox, messageId) => {
    await invoke("voicemail_mark_read", { mailbox, messageId });
    set((s) => {
      const entry = s.byAccount[mailbox.accountId];
      if (!entry) return s;
      return {
        byAccount: {
          ...s.byAccount,
          [mailbox.accountId]: {
            ...entry,
            messages: entry.messages.map((m) => (m.id === messageId ? { ...m, read: true } : m)),
          },
        },
      };
    });
  },

  remove: async (mailbox, messageId) => {
    await invoke("voicemail_delete", { mailbox, messageId });
    set((s) => {
      const entry = s.byAccount[mailbox.accountId];
      if (!entry) return s;
      return {
        byAccount: {
          ...s.byAccount,
          [mailbox.accountId]: { ...entry, messages: entry.messages.filter((m) => m.id !== messageId) },
        },
      };
    });
  },

  loadAudio: async (mailbox, messageId) => {
    return invoke<ArrayBuffer>("voicemail_audio", { mailbox, messageId });
  },

  onMwi: (accountId) => {
    const entry = get().byAccount[accountId];
    if (entry && !entry.loading) void get().refresh(entry.mailbox);
  },
}));
