import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const storage = new Map<string, string>();
vi.stubGlobal("localStorage", {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => void storage.set(k, v),
  removeItem: (k: string) => void storage.delete(k),
});

const mailbox = { accountId: "acc-1", apiUrl: "https://api.ariaroute.com", extension: "1001", domain: "test.ariaroute.com" };
const inbox = { didId: "did-1", number: "+13035550100", enabled: true, lastInboundAt: null };
const conversation = {
  remoteNumber: "+12125550123",
  messageCount: 1,
  lastBody: "See you at 4",
  lastDirection: "inbound",
  lastStatus: "received",
  lastAt: "2026-11-24T10:00:00Z",
  lastHasMedia: false,
  optedOut: false,
};
const received = {
  id: "s1",
  direction: "inbound",
  remoteNumber: "+12125550123",
  body: "See you at 4",
  status: "received",
  autoReply: false,
  errorMessage: null,
  createdAt: "2026-11-24T10:00:00Z",
  attachments: [],
};

async function freshStore() {
  vi.resetModules();
  const mod = await import("./messagingStore");
  return mod.useMessagingStore;
}

beforeEach(() => {
  invoke.mockReset();
  storage.clear();
});

describe("messagingStore", () => {
  it("discovers the account's inboxes and says whether it has messaging", async () => {
    const store = await freshStore();
    invoke.mockResolvedValueOnce([inbox]);
    await store.getState().discover(mailbox);
    expect(invoke).toHaveBeenCalledWith("messaging_inboxes", { mailbox });
    expect(store.getState().hasMessaging("acc-1")).toBe(true);
    expect(store.getState().inboxesOf("acc-1")).toEqual([inbox]);
  });

  it("has no messaging for an account the PBX lists no inbox for, or that fails", async () => {
    const store = await freshStore();
    invoke.mockResolvedValueOnce([]);
    await store.getState().discover(mailbox);
    expect(store.getState().hasMessaging("acc-1")).toBe(false);
    invoke.mockRejectedValueOnce({ kind: "network", message: "down" });
    await store.getState().discover({ ...mailbox, accountId: "acc-2" });
    expect(store.getState().hasMessaging("acc-2")).toBe(false);
    expect(store.getState().byAccount["acc-2"].error?.kind).toBe("network");
  });

  it("loads conversations and counts the unseen ones until a thread is opened", async () => {
    const store = await freshStore();
    invoke.mockResolvedValueOnce([conversation]);
    await store.getState().loadConversations(mailbox, "did-1");
    expect(invoke).toHaveBeenCalledWith("messaging_conversations", { mailbox, didId: "did-1" });
    expect(store.getState().unseen("did-1")).toBe(1);

    invoke.mockResolvedValueOnce({ remoteNumber: "+12125550123", optedOut: false, messages: [received] });
    await store.getState().openThread(mailbox, "did-1", "+12125550123");
    expect(invoke).toHaveBeenCalledWith("messaging_thread", { mailbox, didId: "did-1", number: "+12125550123", before: null });
    expect(store.getState().unseen("did-1")).toBe(0);
    // What was seen survives a restart.
    expect(JSON.parse(storage.get("aria_messages_seen") ?? "{}")).toEqual({ "did-1|+12125550123": "2026-11-24T10:00:00Z" });
  });

  it("sends with a fresh idempotency key and shows the message in the thread and the list", async () => {
    const store = await freshStore();
    invoke.mockResolvedValueOnce({ remoteNumber: "+12125550123", optedOut: false, messages: [received] });
    await store.getState().openThread(mailbox, "did-1", "+12125550123");
    const sent = { ...received, id: "s2", direction: "outbound", body: "On my way", status: "sent", createdAt: "2026-11-24T11:00:00Z" };
    invoke.mockResolvedValueOnce(sent);
    await store.getState().send(mailbox, "did-1", "+12125550123", "On my way");
    const call = invoke.mock.calls.find((c) => c[0] === "messaging_send");
    expect(call?.[1]).toMatchObject({ mailbox, outgoing: { didId: "did-1", to: "+12125550123", body: "On my way" } });
    expect((call?.[1] as { outgoing: { clientKey: string } }).outgoing.clientKey).toMatch(/^[0-9a-f-]{36}$/);
    const thread = store.getState().threads["did-1|+12125550123"];
    expect(thread?.thread.messages.map((m) => m.id)).toEqual(["s2", "s1"]);
    const list = store.getState().conversations["did-1"]?.list ?? [];
    expect(list[0]).toMatchObject({ remoteNumber: "+12125550123", lastBody: "On my way", lastDirection: "outbound" });
  });

  it("rejects a refused send with the PBX's words and keeps the thread as it was", async () => {
    const store = await freshStore();
    invoke.mockResolvedValueOnce({ remoteNumber: "+12125550199", optedOut: true, messages: [] });
    await store.getState().openThread(mailbox, "did-1", "+12125550199");
    invoke.mockRejectedValueOnce({ kind: "rejected", message: "+12125550199 has opted out." });
    await expect(store.getState().send(mailbox, "did-1", "+12125550199", "Hi")).rejects.toMatchObject({ kind: "rejected" });
    expect(store.getState().threads["did-1|+12125550199"]?.thread.messages).toEqual([]);
  });
});
