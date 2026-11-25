import { useEffect } from "react";
import { useAppStore } from "../stores/appStore";
import { useMessagingStore } from "../stores/messagingStore";
import { mailboxFor } from "../utils/voicemail";

/** How often the conversation lists are re-read for the badge while the view is closed. */
const BADGE_POLL_MS = 60_000;

/**
 * Finds out whether the active account has messaging numbers on its PBX
 * (so the Messages item appears by itself) and keeps their conversation
 * lists fresh enough for the unseen badge. The PBX pushes `sms_received`
 * over its WebSocket to browser sessions; the desktop polls instead, as the
 * admin inbox does.
 */
export function useMessaging() {
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const account = accounts.find((a) => a.id === activeAccountId);
  const mailbox = mailboxFor(account);
  const accountId = mailbox?.accountId;
  const apiUrl = mailbox?.apiUrl;
  const extension = mailbox?.extension;
  const domain = mailbox?.domain;
  const inboxes = useMessagingStore((s) => (accountId ? s.byAccount[accountId]?.inboxes : undefined));
  const discover = useMessagingStore((s) => s.discover);
  const loadConversations = useMessagingStore((s) => s.loadConversations);

  useEffect(() => {
    if (!accountId || !apiUrl || !extension || !domain) return;
    void discover({ accountId, apiUrl, extension, domain });
  }, [accountId, apiUrl, extension, domain, discover]);

  const didIds = (inboxes ?? []).map((i) => i.didId).join(",");
  useEffect(() => {
    if (!accountId || !apiUrl || !extension || !domain || !didIds) return;
    const box = { accountId, apiUrl, extension, domain };
    const ids = didIds.split(",");
    const tick = () => {
      for (const didId of ids) void loadConversations(box, didId);
    };
    tick();
    const timer = window.setInterval(tick, BADGE_POLL_MS);
    return () => window.clearInterval(timer);
  }, [accountId, apiUrl, extension, domain, didIds, loadConversations]);
}
