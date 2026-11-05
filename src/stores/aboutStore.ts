import { create } from "zustand";

interface AboutState {
  /** Whether the About dialog is on screen. */
  open: boolean;
  openAbout: () => void;
  closeAbout: () => void;
}

/**
 * The About dialog's visibility. A store rather than local state because it is
 * opened from three places: the macOS app menu (About Aria and Check for
 * Updates…, via `useAppMenu`) and the About row in Settings.
 */
export const useAboutStore = create<AboutState>((set) => ({
  open: false,
  openAbout: () => set({ open: true }),
  closeAbout: () => set({ open: false }),
}));
