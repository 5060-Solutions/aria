import { create } from "zustand";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { log } from "../utils/log";

/**
 * Where Linux users are sent instead of self-updating. AppImage self-update is
 * unreliable and .deb/.rpm users expect their package manager to be in charge,
 * so on Linux we only ever notify. Mirrors the `plugins.updater.endpoints`
 * release in src-tauri/tauri.conf.json.
 */
export const RELEASE_PAGE_URL =
  "https://github.com/5060-Solutions/aria/releases/latest";

export type UpdatePlatform = "macos" | "windows" | "linux" | "unknown";

/**
 * The updater plugin has no platform helper and neither `@tauri-apps/plugin-os`
 * nor `@tauri-apps/api/os` is installed (adding the former would mean touching
 * src-tauri), so read the platform off the webview instead. WebKitGTK reports
 * "X11; Linux ...", WKWebView "Macintosh; Intel Mac OS X ..." and WebView2
 * "Windows NT ...".
 */
function detectPlatform(): UpdatePlatform {
  const raw = navigator.userAgent.toLowerCase();
  if (raw.includes("windows") || raw.includes("win64") || raw.includes("win32")) {
    return "windows";
  }
  if (raw.includes("macintosh") || raw.includes("mac os")) return "macos";
  // Android also reports "linux"; Aria desktop never runs there, but be exact.
  if (raw.includes("linux") && !raw.includes("android")) return "linux";
  return "unknown";
}

export const updatePlatform: UpdatePlatform = detectPlatform();

/** Linux gets the notification but never the in-app install. */
export const canSelfUpdate = updatePlatform !== "linux";

export type UpdateCheckStatus =
  | "idle"
  | "checking"
  | "available"
  | "upToDate"
  | "error";

export type UpdateInstallStatus =
  | "idle"
  | "downloading"
  | "installing"
  | "installed"
  | "error";

interface UpdateState {
  checkStatus: UpdateCheckStatus;
  installStatus: UpdateInstallStatus;
  /** True when the last check was started by the user from Settings. */
  manualCheck: boolean;
  /** Version offered by the update server, once one is available. */
  availableVersion: string | null;
  /** Release notes body from latest.json. */
  releaseNotes: string | null;
  /** Version of the running app. */
  currentVersion: string | null;
  /** Raw error text — only ever surfaced for manual checks / installs. */
  errorMessage: string | null;
  downloadedBytes: number;
  totalBytes: number | null;
  /** Set when the user picks "Later" — suppresses the prompt for this session. */
  dismissed: boolean;

  checkForUpdate: (manual: boolean) => Promise<void>;
  installUpdate: () => Promise<void>;
  /** Relaunch into the installed version. User-initiated only. */
  restartApp: () => Promise<void>;
  openReleasePage: () => Promise<void>;
  dismiss: () => void;
  clearManualResult: () => void;
}

/**
 * The plugin's Update is a native resource handle, so it lives outside the
 * store: it is never rendered and must survive without being cloned.
 */
let pendingUpdate: Update | null = null;

function toMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return String(error);
}

async function readCurrentVersion(): Promise<string | null> {
  try {
    return await getVersion();
  } catch (error) {
    log.warn("[Updater] Could not read app version:", error);
    return null;
  }
}

export const useUpdateStore = create<UpdateState>((set, get) => ({
  checkStatus: "idle",
  installStatus: "idle",
  manualCheck: false,
  availableVersion: null,
  releaseNotes: null,
  currentVersion: null,
  errorMessage: null,
  downloadedBytes: 0,
  totalBytes: null,
  dismissed: false,

  checkForUpdate: async (manual) => {
    if (get().checkStatus === "checking") return;
    // Never interrupt an install that is already running.
    if (get().installStatus === "downloading" || get().installStatus === "installing") {
      return;
    }

    set({
      checkStatus: "checking",
      manualCheck: manual,
      errorMessage: null,
      ...(manual ? { dismissed: false } : {}),
    });

    try {
      const update = await check();
      const currentVersion = update?.currentVersion ?? (await readCurrentVersion());

      // Release the handle from any earlier check before replacing it.
      if (pendingUpdate && pendingUpdate !== update) {
        void pendingUpdate.close().catch(() => {});
      }

      if (!update) {
        pendingUpdate = null;
        set({
          checkStatus: "upToDate",
          availableVersion: null,
          releaseNotes: null,
          currentVersion,
        });
        return;
      }

      pendingUpdate = update;
      set({
        checkStatus: "available",
        availableVersion: update.version,
        releaseNotes: update.body?.trim() ? update.body.trim() : null,
        currentVersion,
        installStatus: "idle",
        downloadedBytes: 0,
        totalBytes: null,
      });
      log.info(`[Updater] Update available: ${update.version}`);
    } catch (error) {
      // A failed automatic check is silent by design — no network on launch,
      // GitHub down, etc. must never greet the user with an error.
      if (manual) {
        set({ checkStatus: "error", errorMessage: toMessage(error) });
      } else {
        set({ checkStatus: "idle" });
      }
      log.warn("[Updater] Check failed:", error);
    }
  },

  /**
   * Restart into the freshly installed version.
   *
   * Separate from `installUpdate` and never automatic: the user presses this
   * once the install has completed, so we can't restart out from under a call
   * they started while the download was running.
   */
  restartApp: async () => {
    try {
      await relaunch();
    } catch (error) {
      log.error("[Updater] Relaunch failed:", error);
      set({ installStatus: "error", errorMessage: toMessage(error) });
    }
  },

  installUpdate: async () => {
    const update = pendingUpdate;
    if (!update || !canSelfUpdate) return;

    set({
      installStatus: "downloading",
      downloadedBytes: 0,
      totalBytes: null,
      errorMessage: null,
    });

    try {
      await update.downloadAndInstall((event: DownloadEvent) => {
        if (event.event === "Started") {
          set({ totalBytes: event.data.contentLength ?? null, downloadedBytes: 0 });
        } else if (event.event === "Progress") {
          set((s) => ({ downloadedBytes: s.downloadedBytes + event.data.chunkLength }));
        } else {
          set({ installStatus: "installing" });
        }
      });
      // On Windows the passive installer terminates the app before we get here.
      set({ installStatus: "installed" });
    } catch (error) {
      log.error("[Updater] Install failed:", error);
      set({ installStatus: "error", errorMessage: toMessage(error) });
    }
  },

  openReleasePage: async () => {
    try {
      await openUrl(RELEASE_PAGE_URL);
    } catch (error) {
      log.error("[Updater] Could not open release page:", error);
      set({ errorMessage: toMessage(error) });
    }
  },

  dismiss: () => set({ dismissed: true }),

  clearManualResult: () =>
    set((s) =>
      s.checkStatus === "upToDate" || s.checkStatus === "error"
        ? { checkStatus: "idle", errorMessage: null }
        : {}
    ),
}));
