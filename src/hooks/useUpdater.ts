import { useEffect } from "react";
import { useUpdateStore } from "../stores/updateStore";

/**
 * Delay before the launch check. SIP registration happens immediately at
 * startup and matters far more than an update check, so we stay out of its way.
 */
const LAUNCH_CHECK_DELAY_MS = 8000;

/** Module-scoped so the check runs once per app session, not per mount. */
let launchCheckScheduled = false;

/**
 * Checks for an update once, a few seconds after launch. Failures are silent —
 * see `checkForUpdate` in the update store.
 */
export function useUpdateOnLaunch() {
  const checkForUpdate = useUpdateStore((s) => s.checkForUpdate);

  useEffect(() => {
    if (launchCheckScheduled) return;
    launchCheckScheduled = true;

    // Deliberately not cleared on unmount: the timer belongs to the session,
    // and cancelling it would skip the check entirely under StrictMode's
    // mount/unmount/remount in development.
    setTimeout(() => {
      void checkForUpdate(false);
    }, LAUNCH_CHECK_DELAY_MS);
  }, [checkForUpdate]);
}
