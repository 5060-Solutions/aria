import { useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "../stores/appStore";
import { meRequest, useCallMoveStore } from "../stores/callMoveStore";
import { messagingMailboxFor } from "../utils/messaging";
import { POLL_MS, contextToReport, parseCallEvent, proximityBody, withSipInstance, type ContextKind } from "../utils/callMove";
import { log } from "../utils/log";
import type { Mailbox } from "../utils/voicemail";

/** How often the computer's lock and idle state is read during a call. */
export const CONTEXT_POLL_MS = 5_000;

/** The active account's PBX mailbox, by value so it does not churn. */
export function useMoveMailbox(): Mailbox | null {
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const json = JSON.stringify(messagingMailboxFor(accounts.find((a) => a.id === activeAccountId)));
  return useMemo(() => JSON.parse(json) as Mailbox | null, [json]);
}

/** Report which network this computer is on, as a hash. */
export async function reportProximity(mailbox: Mailbox): Promise<void> {
  const hash = await invoke<string | null>("network_fingerprint", { mailbox });
  if (!hash) return;
  const urn = await invoke<string>("sip_instance_urn");
  await meRequest(mailbox, "POST", withSipInstance("/me/devices/current/proximity", urn), proximityBody(hash));
}

/**
 * Keeps call moving current: the PBX's call events (carried by the backend's
 * WebSocket, see useMessaging), a poll of the active calls on focus and every
 * 15 s, this device's proximity, and the lock and idle context of a call
 * held here.
 */
export function useCallMove(): void {
  const mailbox = useMoveMailbox();

  useEffect(() => {
    // Another account's calls, devices and registration id are not this one's.
    useCallMoveStore.getState().forAccount(mailbox?.accountId ?? null);
    if (!mailbox) return;
    const store = useCallMoveStore.getState();
    const unlisten = listen<{ accountId: string; event: unknown }>("pbx-call-event", (e) => {
      if (e.payload.accountId !== mailbox.accountId) return;
      const event = parseCallEvent(e.payload.event);
      if (event) useCallMoveStore.getState().applyEvent(event, mailbox);
    });

    // A hidden window does not poll: it refreshes when it is shown again.
    const refresh = () => {
      if (document.visibilityState === "hidden") return;
      void store.refreshCalls(mailbox);
    };
    const proximity = () => {
      void useCallMoveStore
        .getState()
        .ensureCurrent(mailbox)
        .then((id) => (id ? reportProximity(mailbox) : undefined))
        .catch((e) => log.warn("[callMove] proximity not reported:", e));
    };
    refresh();
    proximity();
    void store.loadDevices(mailbox);
    void store.loadHandoff(mailbox);
    const onFocus = () => {
      refresh();
      proximity();
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") onFocus();
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", onFocus);
    window.addEventListener("online", proximity);
    const poll = setInterval(refresh, POLL_MS);
    // About every 60 s while the window is visible.
    const proximityPoll = setInterval(() => {
      if (document.visibilityState !== "hidden") proximity();
    }, 60_000);
    return () => {
      void unlisten.then((fn) => fn());
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("online", proximity);
      clearInterval(poll);
      clearInterval(proximityPoll);
    };
  }, [mailbox]);

  // A suggestion is about the call held here: once no call is, it is stale.
  const hasLive = useAppStore((s) => s.activeCalls.some((c) => c.state !== "ended"));
  useEffect(() => {
    if (!hasLive && useCallMoveStore.getState().suggestion) useCallMoveStore.getState().dismissSuggestion();
  }, [hasLive]);

  // Context: while a call is up on this device, say when the computer locks
  // or the person has gone idle. Once per kind per call.
  const connected = useAppStore((s) => s.activeCalls.some((c) => c.state === "connected" && !c.held));
  useEffect(() => {
    if (!mailbox || !connected) return;
    const sent = new Set<ContextKind>();
    let stopped = false;
    const tick = async () => {
      try {
        const ctx = await invoke<{ locked: boolean; idleSecs: number }>("desktop_context");
        const kind = contextToReport(ctx, sent);
        if (stopped || !kind) return;
        sent.add(kind);
        await useCallMoveStore.getState().reportContext(mailbox, kind);
      } catch (e) {
        log.warn("[callMove] context not reported:", e);
      }
    };
    const timer = setInterval(() => void tick(), CONTEXT_POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [mailbox, connected]);
}
