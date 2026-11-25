import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log } from "../utils/log";
import type { Mailbox } from "../utils/voicemail";
import { notifyText } from "../utils/messageNotification";
import {
  formatNumber,
  isMessagingError,
  isUnread,
  newInbound,
  previewOf,
  wasAnswered,
  type Attachment,
  type Conversation,
  type Inbox,
  type MessagingError,
  type ReadMarkers,
  type SmsMessage,
  type Thread,
} from "../utils/messaging";
import i18n from "../i18n";

const READ_KEY = "aria_sms_read";
const BASELINE_KEY = "aria_sms_baseline";

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Private mode or a full disk: unread state is a convenience.
  }
}

export interface AccountMessaging {
  mailbox: Mailbox;
  inboxes: Inbox[];
  /** By inbox `did_id`, newest first. */
  conversations: Record<string, Conversation[]>;
  /** By `did_id|number`. */
  threads: Record<string, Thread>;
  loading: boolean;
  loaded: boolean;
  error: MessagingError | null;
  /** The PBX event stream is connected; polling is the fallback while it is not. */
  live: boolean;
  /** When this account first loaded texts here: older ones are not "new". */
  baseline: string;
}

/** Which thread is on screen, so its notification is held back. */
export interface OpenThread {
  accountId: string;
  didId: string;
  number: string;
}

interface MessagingStoreState {
  byAccount: Record<string, AccountMessaging>;
  readAt: ReadMarkers;
  open: OpenThread | null;
  setOpen: (open: OpenThread | null) => void;
  /** Reload inboxes and their conversations; raises a notification for a text that is new. */
  refresh: (mailbox: Mailbox) => Promise<void>;
  loadThread: (mailbox: Mailbox, didId: string, number: string) => Promise<Thread>;
  send: (
    mailbox: Mailbox,
    msg: { didId: string; to: string; body: string; attachmentIds: string[]; idempotencyKey: string },
  ) => Promise<SmsMessage>;
  upload: (mailbox: Mailbox, file: File) => Promise<Attachment>;
  loadAttachment: (mailbox: Mailbox, id: string) => Promise<ArrayBuffer>;
  markRead: (accountId: string, didId: string, number: string) => void;
  setLive: (accountId: string, live: boolean) => void;
}

export const threadId = (didId: string, number: string) => `${didId}|${number}`;

function asError(e: unknown): MessagingError {
  return isMessagingError(e) ? e : { kind: "network", message: typeof e === "string" ? e : e instanceof Error ? e.message : String(e) };
}

function blank(mailbox: Mailbox, baseline: string): AccountMessaging {
  return { mailbox, inboxes: [], conversations: {}, threads: {}, loading: false, loaded: false, error: null, live: false, baseline };
}

function baselineFor(accountId: string): string {
  const all = readJson<Record<string, string>>(BASELINE_KEY, {});
  if (all[accountId]) return all[accountId];
  const now = new Date().toISOString();
  writeJson(BASELINE_KEY, { ...all, [accountId]: now });
  return now;
}

export const useMessagingStore = create<MessagingStoreState>((set, get) => {
  const patch = (accountId: string, mailbox: Mailbox, change: (a: AccountMessaging) => Partial<AccountMessaging>) =>
    set((s) => {
      const current = s.byAccount[accountId] ?? blank(mailbox, baselineFor(accountId));
      return { byAccount: { ...s.byAccount, [accountId]: { ...current, mailbox, ...change(current) } } };
    });

  return {
    byAccount: {},
    readAt: readJson<ReadMarkers>(READ_KEY, {}),
    open: null,
    setOpen: (open) => set({ open }),

    refresh: async (mailbox) => {
      const id = mailbox.accountId;
      patch(id, mailbox, () => ({ loading: true }));
      try {
        const inboxes = await invoke<Inbox[]>("messaging_inboxes", { mailbox });
        const lists = await Promise.all(
          inboxes.map(async (i) => [i.did_id, await invoke<Conversation[]>("messaging_conversations", { mailbox, didId: i.did_id })] as const),
        );
        const before = get().byAccount[id];
        const conversations = Object.fromEntries(lists);
        for (const inbox of inboxes) {
          const fresh = newInbound(before?.loaded ? before.conversations[inbox.did_id] : undefined, conversations[inbox.did_id] ?? []);
          for (const c of fresh) announce(id, inbox, c);
        }
        patch(id, mailbox, () => ({ inboxes, conversations, loading: false, loaded: true, error: null }));
        // An open thread follows its conversation: a delivery report or a
        // reply changes it without anyone touching it.
        const open = get().open;
        if (open && open.accountId === id) void get().loadThread(mailbox, open.didId, open.number).catch(() => {});
      } catch (e) {
        const error = asError(e);
        log.warn("[messaging] load failed:", error.kind, error.message);
        patch(id, mailbox, () => ({ loading: false, loaded: true, error }));
      }
    },

    loadThread: async (mailbox, didId, number) => {
      const thread = await invoke<Thread>("messaging_thread", { mailbox, didId, number });
      patch(mailbox.accountId, mailbox, (a) => ({ threads: { ...a.threads, [threadId(didId, number)]: thread } }));
      return thread;
    },

    send: async (mailbox, { didId, to, body, attachmentIds, idempotencyKey }) => {
      let answered = true;
      try {
        return await invoke<SmsMessage>("messaging_send", { mailbox, didId, to, body, attachmentIds, idempotencyKey });
      } catch (e) {
        answered = wasAnswered(e);
        throw e;
      } finally {
        // A success shows in the thread; so does a text the carrier refused
        // (502), which the PBX keeps as failed with its reason. Only a send
        // that never got an answer has nothing new to show.
        if (answered) {
          await get().refresh(mailbox);
          await get().loadThread(mailbox, didId, to).catch(() => {});
        }
      }
    },

    upload: async (mailbox, file) => {
      const bytes = new Uint8Array(await file.arrayBuffer());
      return invoke<Attachment>("messaging_upload", bytes, {
        headers: { "x-mailbox": JSON.stringify(mailbox), "x-file-name": encodeURIComponent(file.name) },
      });
    },

    loadAttachment: (mailbox, id) => invoke<ArrayBuffer>("messaging_attachment", { mailbox, id }),

    markRead: (accountId, didId, number) => {
      const readAt = { ...get().readAt, [`${accountId}|${didId}|${number}`]: new Date().toISOString() };
      writeJson(READ_KEY, readAt);
      set({ readAt });
    },

    setLive: (accountId, live) =>
      set((s) => {
        const a = s.byAccount[accountId];
        return a ? { byAccount: { ...s.byAccount, [accountId]: { ...a, live } } } : s;
      }),
  };
});

/** A desktop notification, unless that thread is open in a focused window. */
function announce(accountId: string, inbox: Inbox, c: Conversation): void {
  const { open } = useMessagingStore.getState();
  const watching =
    open?.accountId === accountId && open.didId === inbox.did_id && open.number === c.remote_number && typeof document !== "undefined" && document.hasFocus();
  if (watching) return;
  const picture = i18n.t("messages.picture");
  void notifyText(
    i18n.t("messages.notificationTitle", { from: formatNumber(c.remote_number) }),
    previewOf(c.last_body, c.last_has_media, picture),
  );
}

/** How many conversations of the account are unread, across its inboxes. */
export function unreadConversations(a: AccountMessaging | undefined, readAt: ReadMarkers): number {
  if (!a) return 0;
  let n = 0;
  for (const [didId, list] of Object.entries(a.conversations)) {
    for (const c of list) {
      if (isUnread(c, readAt[`${a.mailbox.accountId}|${didId}|${c.remote_number}`], a.baseline)) n += 1;
    }
  }
  return n;
}
