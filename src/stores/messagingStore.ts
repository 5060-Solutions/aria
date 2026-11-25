import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log } from "../utils/log";
import type { Mailbox } from "../utils/voicemail";
import { notifyText } from "../utils/messageNotification";
import {
  formatNumber,
  editDraft,
  emptyDraft,
  failedDraft,
  isMessagingError,
  reachedServer,
  sendable,
  threadKey,
  type Draft,
  isUnread,
  newInbound,
  previewOf,
  type Attachment,
  type Conversation,
  type Inbox,
  type MessagingError,
  type ReadMarkers,
  type SmsMessage,
  type Thread,
} from "../utils/messaging";
import i18n from "../i18n";
import { ensureBound } from "../utils/pbxBinding";

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
  /** A failure listing one inbox's conversations; its other data stays. */
  inboxErrors: Record<string, MessagingError>;
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
  /** Drafts by `threadKey`: they survive the thread view being torn down. */
  drafts: Record<string, Draft>;
  /** A thread a notification asked to open; the Messages view takes it. */
  requested: OpenThread | null;
  setRequested: (r: OpenThread | null) => void;
  /** Open the thread of the latest notification if it was raised just now. */
  openFromNotification: (now?: number) => OpenThread | null;
  setDraft: (key: string, patch: { body?: string; attachment?: Attachment | null }) => void;
  /** Send what the thread's draft holds; the outcome lands in the draft. */
  sendDraft: (mailbox: Mailbox, didId: string, number: string) => Promise<void>;
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
  return { mailbox, inboxes: [], conversations: {}, threads: {}, loading: false, loaded: false, error: null, inboxErrors: {}, live: false, baseline };
}

function baselineFor(accountId: string): string {
  const all = readJson<Record<string, string>>(BASELINE_KEY, {});
  if (all[accountId]) return all[accountId];
  const now = new Date().toISOString();
  writeJson(BASELINE_KEY, { ...all, [accountId]: now });
  return now;
}

/** Refreshes in flight per account, and those asked for meanwhile. */
const running = new Map<string, Promise<void>>();
const again = new Set<string>();
/** The newest thread load per thread: an older one finishing later is dropped. */
const threadGeneration = new Map<string, number>();
/** The notification just raised, for a click that arrives as the window gaining focus. */
let lastNotice: { target: OpenThread; at: number } | null = null;
/** How long after a notification a window focus counts as its click. */
const NOTICE_WINDOW_MS = 30_000;

export const useMessagingStore = create<MessagingStoreState>((set, get) => {
  const patch = (accountId: string, mailbox: Mailbox, change: (a: AccountMessaging) => Partial<AccountMessaging>) =>
    set((s) => {
      const current = s.byAccount[accountId] ?? blank(mailbox, baselineFor(accountId));
      return { byAccount: { ...s.byAccount, [accountId]: { ...current, mailbox, ...change(current) } } };
    });

  /** One full reload. Callers go through `refresh`, which never overlaps two. */
  const reload = async (mailbox: Mailbox) => {
    const id = mailbox.accountId;
    patch(id, mailbox, () => ({ loading: true }));
    try {
      await ensureBound(mailbox);
      const inboxes = await invoke<Inbox[]>("messaging_inboxes", { mailbox });
      // One inbox failing must not hide the others, nor wipe what it showed.
      const settled = await Promise.allSettled(
        inboxes.map((i) => invoke<Conversation[]>("messaging_conversations", { mailbox, didId: i.did_id })),
      );
      const before = get().byAccount[id];
      const conversations: Record<string, Conversation[]> = {};
      const inboxErrors: Record<string, MessagingError> = {};
      inboxes.forEach((inbox, n) => {
        const result = settled[n];
        if (result.status === "fulfilled") {
          const fresh = newInbound(before?.loaded ? before.conversations[inbox.did_id] : undefined, result.value);
          for (const c of fresh) announce(id, inbox, c);
          conversations[inbox.did_id] = result.value;
        } else {
          inboxErrors[inbox.did_id] = asError(result.reason);
          conversations[inbox.did_id] = before?.conversations[inbox.did_id] ?? [];
        }
      });
      patch(id, mailbox, () => ({ inboxes, conversations, inboxErrors, loading: false, loaded: true, error: null }));
      // An open thread follows its conversation: a delivery report or a
      // reply changes it without anyone touching it.
      const open = get().open;
      if (open && open.accountId === id) void get().loadThread(mailbox, open.didId, open.number).catch(() => {});
    } catch (e) {
      const error = asError(e);
      log.warn("[messaging] load failed:", error.kind, error.message);
      patch(id, mailbox, () => ({ loading: false, loaded: true, error }));
    }
  };

  return {
    byAccount: {},
    readAt: readJson<ReadMarkers>(READ_KEY, {}),
    open: null,
    drafts: {},
    requested: null,
    setOpen: (open) => set({ open }),
    setRequested: (requested) => set({ requested }),

    openFromNotification: (now = Date.now()) => {
      const notice = lastNotice;
      if (!notice || now - notice.at > NOTICE_WINDOW_MS) return null;
      lastNotice = null;
      const { accountId, didId } = notice.target;
      // Only a thread of an inbox this session was given: the PBX scopes the
      // inbox list to the signed-in tenant and extension.
      const known = get().byAccount[accountId]?.inboxes.some((i) => i.did_id === didId);
      if (!known) return null;
      set({ requested: notice.target });
      return notice.target;
    },

    // Overlapping refreshes (a poll, an event, a send) never run at once: one
    // in flight, and a single follow-up for whatever was asked meanwhile, so
    // an older answer can never land over a newer one.
    refresh: (mailbox) => {
      const id = mailbox.accountId;
      const current = running.get(id);
      if (current) {
        again.add(id);
        return current;
      }
      const run = (async () => {
        do {
          again.delete(id);
          await reload(mailbox);
        } while (again.has(id));
      })().finally(() => running.delete(id));
      running.set(id, run);
      return run;
    },

    loadThread: async (mailbox, didId, number) => {
      const id = threadId(didId, number);
      const key = `${mailbox.accountId}|${id}`;
      const generation = (threadGeneration.get(key) ?? 0) + 1;
      threadGeneration.set(key, generation);
      await ensureBound(mailbox);
      const thread = await invoke<Thread>("messaging_thread", { mailbox, didId, number });
      if (threadGeneration.get(key) === generation) {
        patch(mailbox.accountId, mailbox, (a) => ({ threads: { ...a.threads, [id]: thread } }));
      }
      return thread;
    },

    send: async (mailbox, { didId, to, body, attachmentIds, idempotencyKey }) => {
      let reached = true;
      try {
        return await invoke<SmsMessage>("messaging_send", { mailbox, didId, to, body, attachmentIds, idempotencyKey });
      } catch (e) {
        reached = reachedServer(e);
        throw e;
      } finally {
        // A success shows in the thread; so does a text the carrier refused
        // (502), which the PBX keeps as failed with its reason. Only a send
        // that never got an answer has nothing new to show.
        if (reached) {
          await get().refresh(mailbox);
          await get().loadThread(mailbox, didId, to).catch(() => {});
        }
      }
    },

    setDraft: (key, change) =>
      set((s) => ({ drafts: { ...s.drafts, [key]: editDraft(s.drafts[key] ?? emptyDraft(), change) } })),

    sendDraft: async (mailbox, didId, number) => {
      const key = threadKey(mailbox.accountId, didId, number);
      const draft = get().drafts[key] ?? emptyDraft();
      // One send at a time per thread, even across the view being remounted.
      if (draft.sending) return;
      const body = sendable(draft);
      if (!body && !draft.attachment) return;
      set((s) => ({ drafts: { ...s.drafts, [key]: { ...draft, sending: true, error: null } } }));
      try {
        await get().send(mailbox, {
          didId,
          to: number,
          body,
          attachmentIds: draft.attachment ? [draft.attachment.id] : [],
          idempotencyKey: draft.key,
        });
        set((s) => ({ drafts: { ...s.drafts, [key]: emptyDraft() } }));
      } catch (e) {
        // The draft stays so it can be sent again. After a 4xx the retry is
        // a new request (new key); after a 5xx or no answer it keeps its key,
        // so a send that did go through is not sent twice.
        set((s) => ({ drafts: { ...s.drafts, [key]: failedDraft(s.drafts[key] ?? draft, e) } }));
      }
    },

    upload: async (mailbox, file) => {
      await ensureBound(mailbox);
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
  lastNotice = { target: { accountId, didId: inbox.did_id, number: c.remote_number }, at: Date.now() };
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
