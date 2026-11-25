import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const notifyText = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("../utils/messageNotification", () => ({ notifyText }));
vi.mock("../utils/pbxBinding", () => ({ ensureBound: vi.fn(async () => {}) }));
vi.mock("../i18n", () => ({ default: { t: (k: string, o?: { from?: string }) => (o?.from ? `${k}:${o.from}` : k) } }));

import { threadId, unreadConversations, useMessagingStore } from "./messagingStore";
import type { Conversation } from "../utils/messaging";

const mailbox = { accountId: "acct", apiUrl: "https://api.example.com", extension: "1001", domain: "t.example" };
const inbox = { did_id: "d1", number: "+13035550100", last_inbound_at: null, enabled: true };

function conversation(over: Partial<Conversation> = {}): Conversation {
  return {
    remote_number: "+13035550199",
    message_count: 1,
    last_body: "hello",
    last_direction: "inbound",
    last_status: "received",
    last_at: "2026-11-25T12:00:00Z",
    last_has_media: false,
    opted_out: false,
    ...over,
  };
}

/** The PBX: `conversations` is what the next list returns. */
function pbx(state: { conversations: Conversation[] }) {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "messaging_inboxes") return [inbox];
    if (cmd === "messaging_conversations") return state.conversations;
    if (cmd === "messaging_thread") return { remote_number: "+13035550199", opted_out: false, messages: [] };
    throw new Error(`unexpected ${cmd}`);
  });
}

const store = () => useMessagingStore.getState();

describe("messagingStore", () => {
  beforeEach(() => {
    invoke.mockReset();
    notifyText.mockClear();
    useMessagingStore.setState({ byAccount: {}, readAt: {}, open: null, drafts: {}, requested: null });
  });

  it("loads the inboxes and each one's conversations", async () => {
    pbx({ conversations: [conversation()] });
    await store().refresh(mailbox);
    const a = store().byAccount.acct;
    expect(a.loaded).toBe(true);
    expect(a.error).toBeNull();
    expect(a.inboxes).toEqual([inbox]);
    expect(a.conversations.d1).toHaveLength(1);
  });

  it("does not notify for what is there when the app opens, but does for a text that arrives after", async () => {
    const state = { conversations: [conversation()] };
    pbx(state);
    await store().refresh(mailbox);
    expect(notifyText).not.toHaveBeenCalled();

    state.conversations = [conversation({ last_body: "are you there?", last_at: "2026-11-25T12:05:00Z" })];
    await store().refresh(mailbox);
    expect(notifyText).toHaveBeenCalledTimes(1);
    expect(notifyText).toHaveBeenCalledWith("messages.notificationTitle:+1 303 555 0199", "are you there?");

    // The same text found again (the poll after the event) is not announced twice.
    await store().refresh(mailbox);
    expect(notifyText).toHaveBeenCalledTimes(1);
  });

  it("keeps the notification back for the thread on screen", async () => {
    vi.stubGlobal("document", { hasFocus: () => true });
    const state = { conversations: [conversation()] };
    pbx(state);
    await store().refresh(mailbox);
    store().setOpen({ accountId: "acct", didId: "d1", number: "+13035550199" });
    state.conversations = [conversation({ last_at: "2026-11-25T12:05:00Z" })];
    await store().refresh(mailbox);
    expect(notifyText).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("keeps a load failure where the view can show it", async () => {
    invoke.mockRejectedValue({ kind: "unauthorized", message: "no" });
    await store().refresh(mailbox);
    expect(store().byAccount.acct.error).toEqual({ kind: "unauthorized", message: "no" });
    expect(store().byAccount.acct.loaded).toBe(true);
  });

  it("counts unread conversations until the thread is opened", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date("2026-11-25T12:00:00Z"));
      pbx({ conversations: [conversation({ last_at: "2026-11-25T12:01:00Z" })] });
      await store().refresh(mailbox);
      expect(unreadConversations(store().byAccount.acct, store().readAt)).toBe(1);
      vi.setSystemTime(new Date("2026-11-25T12:05:00Z"));
      store().markRead("acct", "d1", "+13035550199");
      expect(unreadConversations(store().byAccount.acct, store().readAt)).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  describe("send", () => {
    const msg = { didId: "d1", to: "+13035550199", body: "hi", attachmentIds: [], idempotencyKey: "k" };

    it("passes the idempotency key and reloads the thread", async () => {
      pbx({ conversations: [] });
      invoke.mockImplementationOnce(async () => ({ id: "m1" }));
      const sent = await store().send(mailbox, msg);
      expect(sent).toEqual({ id: "m1" });
      expect(invoke).toHaveBeenNthCalledWith(1, "messaging_send", expect.objectContaining({ idempotencyKey: "k", to: "+13035550199" }));
      expect(invoke.mock.calls.map((c) => c[0])).toContain("messaging_thread");
    });

    it("reloads after the PBX refuses (a failed text is kept in the thread) and rethrows its sentence", async () => {
      pbx({ conversations: [] });
      const refusal = { kind: "rejected", message: "Carrier said no.", status: 502 };
      invoke.mockImplementationOnce(async () => {
        throw refusal;
      });
      await expect(store().send(mailbox, msg)).rejects.toEqual(refusal);
      expect(invoke.mock.calls.map((c) => c[0])).toContain("messaging_thread");
    });

    it("does not reload when the PBX could not be reached", async () => {
      invoke.mockImplementation(async () => {
        throw { kind: "network", message: "down" };
      });
      await expect(store().send(mailbox, msg)).rejects.toMatchObject({ kind: "network" });
      expect(invoke).toHaveBeenCalledTimes(1);
    });
  });

  describe("refresh", () => {
    it("keeps the other inboxes when one fails, and what the failed one showed", async () => {
      const inbox2 = { ...inbox, did_id: "d2", number: "+13035550111" };
      let failD2 = false;
      invoke.mockImplementation(async (cmd: string, args: { didId?: string }) => {
        if (cmd === "messaging_inboxes") return [inbox, inbox2];
        if (cmd === "messaging_conversations") {
          if (args.didId === "d2" && failD2) throw { kind: "server", message: "boom", status: 500 };
          return [conversation({ last_body: args.didId })];
        }
        return { remote_number: "x", opted_out: false, messages: [] };
      });
      await store().refresh(mailbox);
      failD2 = true;
      await store().refresh(mailbox);
      const a = store().byAccount.acct;
      expect(a.error).toBeNull();
      expect(a.inboxErrors.d2.message).toBe("boom");
      expect(a.inboxErrors.d1).toBeUndefined();
      expect(a.conversations.d1[0].last_body).toBe("d1");
      expect(a.conversations.d2[0].last_body).toBe("d2");
    });

    it("never runs two at once; overlapping asks share one follow-up and the newest answer stands", async () => {
      let running = 0;
      let peak = 0;
      let lists = 0;
      invoke.mockImplementation(async (cmd: string) => {
        if (cmd === "messaging_inboxes") {
          running += 1;
          peak = Math.max(peak, running);
          await new Promise((r) => setTimeout(r, 5));
          running -= 1;
          return [inbox];
        }
        if (cmd === "messaging_conversations") {
          lists += 1;
          return [conversation({ last_body: `list ${lists}` })];
        }
        return { remote_number: "x", opted_out: false, messages: [] };
      });
      await Promise.all([store().refresh(mailbox), store().refresh(mailbox), store().refresh(mailbox)]);
      expect(peak).toBe(1);
      expect(lists).toBe(2);
      expect(store().byAccount.acct.conversations.d1[0].last_body).toBe("list 2");
    });
  });

  describe("drafts", () => {
    const key = "acct|d1|+13035550199";
    const sendCalls = () => invoke.mock.calls.filter((c) => c[0] === "messaging_send").map((c) => c[1]);

    function pbxSending(outcomes: Array<() => unknown>) {
      invoke.mockImplementation(async (cmd: string) => {
        if (cmd === "messaging_send") return outcomes.shift()?.();
        if (cmd === "messaging_inboxes") return [inbox];
        if (cmd === "messaging_conversations") return [];
        return { remote_number: "+13035550199", opted_out: false, messages: [] };
      });
    }

    it("keeps the draft in the store, with a key that changes only when what is sent does", () => {
      store().setDraft(key, { body: "hel" });
      const first = store().drafts[key].key;
      store().setDraft(key, { body: "hel" });
      expect(store().drafts[key].key).toBe(first);
      store().setDraft(key, { body: "hello" });
      const second = store().drafts[key].key;
      expect(second).not.toBe(first);
      store().setDraft(key, { attachment: { id: "a1", content_type: "image/png", size_bytes: 1 } });
      expect(store().drafts[key].key).not.toBe(second);
    });

    it("sends the trimmed body once, then clears the draft", async () => {
      pbxSending([() => ({ id: "m1" })]);
      store().setDraft(key, { body: "  hi there \n" });
      const sentKey = store().drafts[key].key;
      await store().sendDraft(mailbox, "d1", "+13035550199");
      expect(sendCalls()).toEqual([expect.objectContaining({ body: "hi there", idempotencyKey: sentKey })]);
      expect(store().drafts[key].body).toBe("");
      expect(store().drafts[key].key).not.toBe(sentKey);
    });

    it("keeps the key after a 5xx and after no answer, and renews it after a 4xx", async () => {
      store().setDraft(key, { body: "hi" });
      const k = store().drafts[key].key;
      pbxSending([
        () => {
          throw { kind: "server", message: "carrier down", status: 502 };
        },
        () => {
          throw { kind: "network", message: "offline" };
        },
        () => {
          throw { kind: "rejected", message: "opted out", status: 422, field: "to" };
        },
      ]);
      await store().sendDraft(mailbox, "d1", "+13035550199");
      expect(store().drafts[key]).toMatchObject({ key: k, error: "carrier down", sending: false, body: "hi" });
      await store().sendDraft(mailbox, "d1", "+13035550199");
      expect(store().drafts[key]).toMatchObject({ key: k, error: "offline" });
      await store().sendDraft(mailbox, "d1", "+13035550199");
      expect(store().drafts[key].key).not.toBe(k);
      expect(store().drafts[key].error).toBe("opted out");
      expect(sendCalls().map((c) => (c as { idempotencyKey: string }).idempotencyKey).slice(0, 2)).toEqual([k, k]);
    });

    it("does not send twice while one is in flight, even if the view asks again", async () => {
      let release: (v: unknown) => void = () => {};
      invoke.mockImplementation(async (cmd: string) => {
        if (cmd === "messaging_send") return new Promise((r) => (release = r));
        if (cmd === "messaging_inboxes") return [inbox];
        if (cmd === "messaging_conversations") return [];
        return { remote_number: "x", opted_out: false, messages: [] };
      });
      store().setDraft(key, { body: "hi" });
      const first = store().sendDraft(mailbox, "d1", "+13035550199");
      await store().sendDraft(mailbox, "d1", "+13035550199");
      expect(sendCalls()).toHaveLength(1);
      release({ id: "m1" });
      await first;
    });
  });

  describe("notification click", () => {
    it("opens the thread of a notification just raised, for a known inbox only", async () => {
      const state = { conversations: [conversation()] };
      pbx(state);
      await store().refresh(mailbox);
      state.conversations = [conversation({ last_at: "2026-11-25T12:05:00Z" })];
      await store().refresh(mailbox);
      expect(store().openFromNotification()).toEqual({ accountId: "acct", didId: "d1", number: "+13035550199" });
      expect(store().requested).not.toBeNull();
      // Used up.
      expect(store().openFromNotification()).toBeNull();
    });

    it("ignores a click long after the notification", async () => {
      const state = { conversations: [conversation()] };
      pbx(state);
      await store().refresh(mailbox);
      state.conversations = [conversation({ last_at: "2026-11-25T12:06:00Z" })];
      await store().refresh(mailbox);
      expect(store().openFromNotification(Date.now() + 60_000)).toBeNull();
    });
  });

  it("keys a thread by number within an inbox", () => {
    expect(threadId("d1", "+1")).toBe("d1|+1");
  });
});
