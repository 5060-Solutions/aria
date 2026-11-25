import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const notifyText = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("../utils/messageNotification", () => ({ notifyText }));
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
    useMessagingStore.setState({ byAccount: {}, readAt: {}, open: null });
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

  it("keys a thread by number within an inbox", () => {
    expect(threadId("d1", "+1")).toBe("d1|+1");
  });
});
