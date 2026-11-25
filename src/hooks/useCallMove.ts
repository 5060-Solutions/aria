import { useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "../stores/appStore";
import { meRequest, useCallMoveStore } from "../stores/callMoveStore";
import { messagingMailboxFor } from "../utils/messaging";
import { POLL_MS, contextBody, contextToReport, parseCallEvent, proximityBody, type ContextKind } from "../utils/callMove";
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
  const hash = await invoke<string | null>("network_fingerprint");
  if (!hash) return;
  await meRequest(mailbox, "POST", "/me/devices/current/proximity", proximityBody(hash));
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
    if (!mailbox) return;
    const store = useCallMoveStore.getState();
    const unlisten = listen<{ accountId: string; event: unknown }>("pbx-call-event", (e) => {
      if (e.payload.accountId !== mailbox.accountId) return;
      const event = parseCallEvent(e.payload.event);
      if (event) useCallMoveStore.getState().applyEvent(event, mailbox);
    });

    const refresh = () => {
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
    window.addEventListener("focus", onFocus);
    window.addEventListener("online", proximity);
    const poll = setInterval(refresh, POLL_MS);
    return () => {
      void unlisten.then((fn) => fn());
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("online", proximity);
      clearInterval(poll);
    };
  }, [mailbox]);

  // A move that is not answered in time is forgotten.
  const pending = useCallMoveStore((s) => s.pending);
  useEffect(() => {
    if (!pending) return;
    const timer = setTimeout(() => useCallMoveStore.getState().clearPending(), Math.max(0, pending.expiresAt - Date.now()));
    return () => clearTimeout(timer);
  }, [pending]);

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
        await meRequest(mailbox, "POST", "/me/devices/current/context", contextBody(kind));
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
