import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { log } from "../utils/log";

/**
 * Monitors network state and app visibility to proactively detect
 * stale SIP registrations after sleep/hibernate/network changes.
 *
 * On wake or network recovery, immediately probes registration health
 * via an OPTIONS ping rather than waiting for the next keepalive cycle.
 */
export function useNetworkMonitor() {
  const lastProbe = useRef(0);

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
