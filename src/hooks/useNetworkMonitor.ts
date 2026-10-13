import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../stores/appStore";
import { isAuthFailure, registerAccounts } from "./useSip";
import { log } from "../utils/log";

/**
 * Monitors network state and app visibility to proactively detect
 * stale SIP registrations after sleep/hibernate/network changes.
 *
 * On wake or network recovery, immediately probes registration health
 * via an OPTIONS ping rather than waiting for the next keepalive cycle.
 */
/**
 * Minimum time between re-registration attempts from here. Probes fire on every
 * wake, reconnect and window focus; an account that keeps failing must not be
 * retried that often.
 */
const REREGISTER_INTERVAL_MS = 5 * 60 * 1000;

export function useNetworkMonitor() {
  const lastProbe = useRef(0);
  const lastReregister = useRef(0);

  useEffect(() => {
    // Debounce: don't probe more than once every 3 seconds
    const probe = (reason: string) => {
      const now = Date.now();
      if (now - lastProbe.current < 3000) return;
      lastProbe.current = now;
      log.info(`[NetworkMonitor] Probing registration health: ${reason}`);
      invoke("probe_registration_health")
        .then(() => {
          // After probing, re-subscribe to BLF/presence if we had any
          invoke("process_pending_resubscriptions").catch(() => {});
        })
        .catch((e) => log.error("[NetworkMonitor] Probe failed:", e));

      // Accounts that gave up — e.g. launched before the network was up — get
      // another go. Not "reconnecting" ones, which the backend is already
      // retrying, and never a rejected password (see isAuthFailure).
      const { accounts, accountStates } = useAppStore.getState();
      const stalled = accounts.filter((a) => {
        const st = accountStates[a.id];
        return (
          a.enabled &&
          st?.registrationState === "error" &&
          !isAuthFailure(st.registrationError)
        );
      });
      if (stalled.length > 0 && now - lastReregister.current >= REREGISTER_INTERVAL_MS) {
        lastReregister.current = now;
        log.info("[NetworkMonitor] Re-registering failed accounts:", stalled.map((a) => a.id));
        void registerAccounts(stalled);
      }
    };

    // 1. Visibility change — fires on wake from sleep/hibernate
    const handleVisibility = () => {
      if (document.visibilityState === "visible") {
        // Small delay to let the network stack recover after wake
        setTimeout(() => probe("visibility-change"), 500);
      }
    };

    // 2. Online/offline events — fires on network state changes
    const handleOnline = () => probe("network-online");
    const handleOffline = () => {
      log.warn("[NetworkMonitor] Network offline detected");
    };

    // 3. Window focus — catches cases visibility change misses
    const handleFocus = () => {
      // Only probe if it's been a while since last probe
      const now = Date.now();
      if (now - lastProbe.current > 30000) {
        probe("window-focus");
      }
    };

    document.addEventListener("visibilitychange", handleVisibility);
    window.addEventListener("online", handleOnline);
    window.addEventListener("offline", handleOffline);
    window.addEventListener("focus", handleFocus);

    return () => {
      document.removeEventListener("visibilitychange", handleVisibility);
      window.removeEventListener("online", handleOnline);
      window.removeEventListener("offline", handleOffline);
      window.removeEventListener("focus", handleFocus);
    };
  }, []);
}
