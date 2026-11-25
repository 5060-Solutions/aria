import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { errorText } from "./toastStore";
import { log } from "../utils/log";
import { isVoicemailError, type Mailbox, type VoicemailError } from "../utils/voicemail";
import { mergeThread, sortByLastAt, unseenCount, type Conversation, type Inbox, type SmsMessage, type Thread } from "../utils/messaging";

/** The structured error every PBX command rejects with (`PbxError` in Rust). */
export type PbxError = VoicemailError | { kind: "rejected"; message: string };

interface InboxesState {
  inboxes: Inbox[];
  loading: boolean;
  loaded: boolean;
  error: PbxError | null;
}

interface ConversationsState {
  list: Conversation[];
  loading: boolean;
  loaded: boolean;
  error: PbxError | null;
}

interface ThreadState {
  thread: Thread;
  loading: boolean;
  error: PbxError | null;
  /** The last page was shorter than a page: nothing older to load. */
  exhausted: boolean;
}

interface MessagingStoreState {
  byAccount: Record<string, InboxesState>;
  /** Per inbox (`didId`). */
  conversations: Record<string, ConversationsState>;
  /** Per `${didId}|${remoteNumber}`. */
  threads: Record<string, ThreadState>;
  /** `${didId}|${remoteNumber}` to the newest `lastAt` shown while open. */
  seen: Record<string, string>;

  /** Ask the PBX which messaging numbers the account may use. */
  discover: (mailbox: Mailbox) => Promise<void>;
  hasMessaging: (accountId: string) => boolean;
  inboxesOf: (accountId: string) => Inbox[];
  loadConversations: (mailbox: Mailbox, didId: string) => Promise<void>;
  /** Load (or reload) a thread and record it as seen. */
  openThread: (mailbox: Mailbox, didId: string, number: string) => Promise<void>;
  loadOlder: (mailbox: Mailbox, didId: string, number: string) => Promise<void>;
  /** Send a text; rejects with the PBX's words when it is refused. */
  send: (mailbox: Mailbox, didId: string, to: string, body: string) => Promise<SmsMessage>;
  attachment: (mailbox: Mailbox, attachmentId: string) => Promise<ArrayBuffer>;
  /** Conversations on an inbox with an inbound text newer than last opened. */
  unseen: (didId: string) => number;
}

const SEEN_KEY = "aria_messages_seen";
const PAGE = 100;

function loadSeen(): Record<string, string> {
  try {
    const raw = localStorage.getItem(SEEN_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    return typeof parsed === "object" && parsed !== null ? (parsed as Record<string, string>) : {};
  } catch {
    return {};
  }
}

function saveSeen(seen: Record<string, string>) {
  try {
    localStorage.setItem(SEEN_KEY, JSON.stringify(seen));
  } catch {
    // Storage full or unavailable: the badge is the only thing lost.
  }
}

function asPbxError(e: unknown): PbxError {
  return isVoicemailError(e) ? (e as PbxError) : { kind: "network", message: errorText(e) };
}

export function threadKey(didId: string, number: string): string {
  return `${didId}|${number}`;
}

/** The conversation row a message (received or just sent) stands for. */
function rowFor(message: SmsMessage, have: Conversation | undefined): Conversation {
  return {
    remoteNumber: message.remoteNumber,
    messageCount: (have?.messageCount ?? 0) + 1,
    lastBody: message.body,
    lastDirection: message.direction,
    lastStatus: message.status,
    lastAt: message.createdAt,
    lastHasMedia: message.attachments.length > 0,
    optedOut: have?.optedOut ?? false,
  };
}

/**
 * Texts on the account's messaging numbers, through the PBX API. Only an
 * account whose PBX lists an inbox for it gets a Messages view; the PBX
 * keeps no read flag, so "unseen" is this store's own record.
 */
export const useMessagingStore = create<MessagingStoreState>((set, get) => ({
  byAccount: {},
  conversations: {},
  threads: {},
  seen: loadSeen(),

  discover: async (mailbox) => {
    const id = mailbox.accountId;
    set((s) => ({
      byAccount: { ...s.byAccount, [id]: { ...(s.byAccount[id] ?? { inboxes: [], loaded: false, error: null }), loading: true } },
    }));
    try {
      const inboxes = await invoke<Inbox[]>("messaging_inboxes", { mailbox });
      set((s) => ({ byAccount: { ...s.byAccount, [id]: { inboxes, loading: false, loaded: true, error: null } } }));
    } catch (e) {
      const error = asPbxError(e);
      log.warn("[messages] inboxes failed:", error.kind, error.message);
      set((s) => ({
        byAccount: { ...s.byAccount, [id]: { inboxes: s.byAccount[id]?.inboxes ?? [], loading: false, loaded: true, error } },
      }));
    }
  },

  hasMessaging: (accountId) => (get().byAccount[accountId]?.inboxes.length ?? 0) > 0,

  inboxesOf: (accountId) => get().byAccount[accountId]?.inboxes ?? [],

  loadConversations: async (mailbox, didId) => {
    set((s) => ({
      conversations: {
        ...s.conversations,
        [didId]: { ...(s.conversations[didId] ?? { list: [], loaded: false, error: null }), loading: true },
      },
    }));
    try {
      const list = sortByLastAt(await invoke<Conversation[]>("messaging_conversations", { mailbox, didId }));
      set((s) => ({ conversations: { ...s.conversations, [didId]: { list, loading: false, loaded: true, error: null } } }));
    } catch (e) {
      const error = asPbxError(e);
      log.warn("[messages] conversations failed:", error.kind, error.message);
      set((s) => ({
        conversations: {
          ...s.conversations,
          [didId]: { list: s.conversations[didId]?.list ?? [], loading: false, loaded: true, error },
        },
      }));
    }
  },

  openThread: async (mailbox, didId, number) => {
    const key = threadKey(didId, number);
    set((s) => ({
      threads: {
        ...s.threads,
        [key]: {
          ...(s.threads[key] ?? { thread: { remoteNumber: number, optedOut: false, messages: [] }, exhausted: false }),
          loading: true,
          error: null,
        },
      },
    }));
    try {
      const thread = await invoke<Thread>("messaging_thread", { mailbox, didId, number, before: null });
      const newest = thread.messages[0]?.createdAt ?? get().conversations[didId]?.list.find((c) => c.remoteNumber === number)?.lastAt;
      set((s) => {
        const seen = newest ? { ...s.seen, [key]: newest } : s.seen;
        if (newest) saveSeen(seen);
        return {
          seen,
          threads: {
            ...s.threads,
            [key]: {
              thread: { ...thread, messages: mergeThread(s.threads[key]?.thread.messages ?? [], thread.messages) },
              loading: false,
              error: null,
              exhausted: thread.messages.length < PAGE,
            },
          },
        };
      });
    } catch (e) {
      const error = asPbxError(e);
      log.warn("[messages] thread failed:", error.kind, error.message);
      set((s) => {
        const had = s.threads[key];
        return {
          threads: {
            ...s.threads,
            [key]: { thread: had?.thread ?? { remoteNumber: number, optedOut: false, messages: [] }, loading: false, error, exhausted: had?.exhausted ?? false },
          },
        };
      });
    }
  },

  loadOlder: async (mailbox, didId, number) => {
    const key = threadKey(didId, number);
    const have = get().threads[key];
    const oldest = have?.thread.messages[have.thread.messages.length - 1];
    if (!have || have.loading || have.exhausted || !oldest) return;
    set((s) => ({ threads: { ...s.threads, [key]: { ...have, loading: true } } }));
    try {
      const page = await invoke<Thread>("messaging_thread", { mailbox, didId, number, before: oldest.createdAt });
      set((s) => {
        const current = s.threads[key] ?? have;
        return {
          threads: {
            ...s.threads,
            [key]: {
              ...current,
              thread: { ...current.thread, messages: mergeThread(current.thread.messages, page.messages) },
              loading: false,
              exhausted: page.messages.length < PAGE,
            },
          },
        };
      });
    } catch (e) {
      const error = asPbxError(e);
      log.warn("[messages] older page failed:", error.kind, error.message);
      set((s) => ({ threads: { ...s.threads, [key]: { ...(s.threads[key] ?? have), loading: false, error } } }));
    }
  },

  send: async (mailbox, didId, to, body) => {
    const outgoing = { didId, to, body, clientKey: crypto.randomUUID() };
    let sent: SmsMessage;
    try {
      sent = await invoke<SmsMessage>("messaging_send", { mailbox, outgoing });
    } catch (e) {
      throw asPbxError(e);
    }
    const key = threadKey(didId, to);
    set((s) => {
      const had = s.threads[key];
      const list = s.conversations[didId]?.list ?? [];
      const row = rowFor(sent, list.find((c) => c.remoteNumber === to));
      const seen = { ...s.seen, [key]: sent.createdAt };
      saveSeen(seen);
      return {
        seen,
        threads: {
          ...s.threads,
          [key]: {
            thread: {
              remoteNumber: to,
              optedOut: had?.thread.optedOut ?? false,
              messages: mergeThread(had?.thread.messages ?? [], [sent]),
            },
            loading: false,
            error: null,
            exhausted: had?.exhausted ?? true,
          },
        },
        conversations: {
          ...s.conversations,
          [didId]: {
            ...(s.conversations[didId] ?? { loading: false, loaded: true, error: null }),
            list: sortByLastAt([row, ...list.filter((c) => c.remoteNumber !== to)]),
          },
        },
      };
    });
    return sent;
  },

  attachment: (mailbox, attachmentId) => invoke<ArrayBuffer>("messaging_attachment", { mailbox, attachmentId }),

  unseen: (didId) => {
    const list = get().conversations[didId]?.list ?? [];
    const seen = get().seen;
    const forInbox: Record<string, string> = {};
    for (const c of list) {
      const at = seen[threadKey(didId, c.remoteNumber)];
      if (at) forInbox[c.remoteNumber] = at;
    }
    return unseenCount(list, forInbox);
  },
}));
