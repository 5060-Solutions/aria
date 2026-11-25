import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { log } from "../utils/log";
import type { MessageCategory } from "../utils/sipMessage";
import type { AppInfo, DiagnosticLog, SystemStatus } from "../components/diagnostics/types";

/** The backend keeps this many; the window shows what it has. */
const STATUS_POLL_MS = 2000;
const RESYNC_MS = 15_000;

interface DiagnosticsState {
  logs: DiagnosticLog[];
  status: SystemStatus | null;
  appInfo: AppInfo | null;
  docked: boolean;
  /** Keep the newest message in view as they arrive. */
  follow: boolean;
  search: string;
  categories: Set<MessageCategory>;
  callId: string | null;
  view: "messages" | "ladder";
  showStatus: boolean;
  selectedAccount: string | null;
  expanded: Set<string>;

  /** Load everything and subscribe; returns the function that stops it. */
  start: () => () => void;
  clear: () => Promise<void>;
  setDocked: (docked: boolean) => Promise<void>;
  setFollow: (follow: boolean) => void;
  setSearch: (search: string) => void;
  toggleCategory: (category: MessageCategory) => void;
  setCallId: (callId: string | null) => void;
  setView: (view: "messages" | "ladder") => void;
  toggleStatus: () => void;
  selectAccount: (accountId: string | null) => void;
  toggleExpanded: (key: string) => void;
}

export const useDiagnosticsStore = create<DiagnosticsState>((set) => ({
  logs: [],
  status: null,
  appInfo: null,
  docked: true,
  follow: true,
  search: "",
  categories: new Set<MessageCategory>(["calls", "register", "keepalive", "other"]),
  callId: null,
  view: "messages",
  showStatus: true,
  selectedAccount: null,
  expanded: new Set<string>(),

  start: () => {
    let stopped = false;
    const resync = async () => {
      try {
        const logs = await invoke<DiagnosticLog[]>("get_sip_diagnostics");
        if (!stopped) set({ logs });
      } catch (e) {
        log.warn("[diagnostics] could not load the trace:", e);
      }
    };
    const status = async () => {
      try {
        const s = await invoke<SystemStatus | null>("get_system_status");
        if (!stopped) set({ status: s });
      } catch {
        // The backend is busy; the next poll will do.
      }
    };
    void resync();
    void status();
    invoke<AppInfo>("get_app_info")
      .then((appInfo) => set({ appInfo }))
      .catch((e) => log.warn("[diagnostics] app info:", e));
    invoke<boolean>("diagnostics_docked")
      .then((docked) => set({ docked }))
      .catch(() => {});

    const unlistenLog = listen<DiagnosticLog>("sip-diagnostic", (event) => {
      // Appended from the event itself: a full reload per message made the
      // list flicker and the backend busy during a burst of retransmissions.
      set((s) => ({ logs: [...s.logs, event.payload] }));
    });
    const unlistenDock = listen<boolean>("diagnostics-dock", (event) => set({ docked: event.payload }));
    const statusTimer = setInterval(status, STATUS_POLL_MS);
    const resyncTimer = setInterval(resync, RESYNC_MS);
    return () => {
      stopped = true;
      clearInterval(statusTimer);
      clearInterval(resyncTimer);
      unlistenLog.then((f) => f());
      unlistenDock.then((f) => f());
    };
  },

  clear: async () => {
    try {
      await invoke("clear_sip_diagnostics");
    } catch (e) {
      log.warn("[diagnostics] clear failed:", e);
    }
    set({ logs: [], expanded: new Set(), callId: null });
  },

  setDocked: async (docked) => {
    set({ docked });
    try {
      await invoke("diagnostics_set_docked", { docked });
    } catch (e) {
      log.warn("[diagnostics] dock toggle failed:", e);
    }
  },

  setFollow: (follow) => set({ follow }),
  setSearch: (search) => set({ search }),
  toggleCategory: (category) =>
    set((s) => {
      const next = new Set(s.categories);
      if (next.has(category)) {
        if (next.size > 1) next.delete(category);
      } else {
        next.add(category);
      }
      return { categories: next };
    }),
  setCallId: (callId) => set({ callId }),
  setView: (view) => set({ view }),
  toggleStatus: () => set((s) => ({ showStatus: !s.showStatus })),
  selectAccount: (selectedAccount) => set({ selectedAccount, expanded: new Set() }),
  toggleExpanded: (key) =>
    set((s) => {
      const next = new Set(s.expanded);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return { expanded: next };
    }),
}));
