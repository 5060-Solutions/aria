import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAboutStore } from "../stores/aboutStore";
import { useAppStore } from "../stores/appStore";
import { useUpdateStore } from "../stores/updateStore";
import { log } from "../utils/log";

/**
 * The macOS app menu's own items (src-tauri/src/menu.rs). The backend brings
 * the main window forward and emits one of these; the work happens here,
 * where the About dialog, the updater and Settings live. Windows and Linux
 * have no app menu, so these never fire there.
 */
export const MENU_EVENTS = {
  about: "menu-about",
  checkForUpdates: "menu-check-updates",
  settings: "menu-settings",
} as const;

/**
 * "Check for Updates…" opens About and runs a manual check in it, so every
 * outcome has somewhere to show: checking, up to date, an update with its
 * Install button, or the error. The automatic checks stay silent.
 */
export function checkForUpdatesFromMenu() {
  useAboutStore.getState().openAbout();
  void useUpdateStore.getState().checkForUpdate(true);
}

export function useAppMenu() {
  useEffect(() => {
    const subscriptions = [
      listen(MENU_EVENTS.about, () => useAboutStore.getState().openAbout()),
      listen(MENU_EVENTS.checkForUpdates, checkForUpdatesFromMenu),
      listen(MENU_EVENTS.settings, () => {
        useAboutStore.getState().closeAbout();
        useAppStore.getState().setCurrentView("settings");
      }),
    ];

    return () => {
      for (const subscription of subscriptions) {
        subscription
          .then((unlisten) => unlisten())
          .catch((e) => log.warn("[Menu] Could not stop listening:", e));
      }
    };
  }, []);
}
