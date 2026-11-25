import { useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "../stores/appStore";
import { useMessagingStore } from "../stores/messagingStore";
import { messagingMailboxFor } from "../utils/messaging";
import { log } from "../utils/log";
import { ensureBound } from "../utils/pbxBinding";
import type { Mailbox } from "../utils/voicemail";

/** How often texts are checked for while the PBX event stream is down. */
export const POLL_MS = 15_000;

/**
 * Keeps the active account's texts current: loads them, holds the PBX event
 * stream open (a `sms_received` event triggers a reload) and, while that
 * stream is down, polls every 15 seconds. A new inbound text found by either
 * raises a desktop notification (see the store).
 */
export function useMessaging(): void {
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const account = accounts.find((a) => a.id === activeAccountId);
  // By value: the account object is replaced whenever its registration state
  // changes, and that must not reopen the event stream.
  const mailboxJson = JSON.stringify(messagingMailboxFor(account));
  const mailbox = useMemo<Mailbox | null>(() => JSON.parse(mailboxJson) as Mailbox | null, [mailboxJson]);
  const live = useMessagingStore((s) => (mailbox ? s.byAccount[mailbox.accountId]?.live : undefined)) ?? false;
  const refresh = useMessagingStore((s) => s.refresh);
  const setLive = useMessagingStore((s) => s.setLive);

  // Events from the backend's stream.
  useEffect(() => {
    if (!mailbox) return;
    const accountId = mailbox.accountId;
    const unlisten = [
      listen<{ accountId: string }>("messaging-sms", (e) => {
        if (e.payload.accountId === accountId) void refresh(mailbox);
      }),
      listen<{ accountId: string; live: boolean }>("messaging-live", (e) => {
        if (e.payload.accountId === accountId) setLive(accountId, e.payload.live);
      }),
    ];
    ensureBound(mailbox)
      .then(() => invoke("messaging_listen", { mailbox }))
      .catch((e) => log.warn("[messaging] event stream not started:", e));
    return () => {
      unlisten.forEach((p) => void p.then((fn) => fn()));
      invoke("messaging_stop", { accountId }).catch(() => {});
      setLive(accountId, false);
    };
  }, [mailbox, refresh, setLive]);

  // A notification's click brings the window forward: the desktop plugin
  // reports no click on macOS or Windows, so the window gaining focus just
  // after one is taken as it, and opens that thread (see the store).
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  useEffect(() => {
    const onFocus = () => {
      if (useMessagingStore.getState().openFromNotification()) setCurrentView("messages");
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [setCurrentView]);

  // The first load, and the fallback while there is no stream.
  useEffect(() => {
    if (!mailbox) return;
    void refresh(mailbox);
    if (live) return;
    const timer = setInterval(() => void refresh(mailbox), POLL_MS);
    return () => clearInterval(timer);
  }, [mailbox, live, refresh]);
}
