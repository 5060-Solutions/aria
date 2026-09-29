import { useEffect } from "react";
import { useUpdateStore } from "../stores/updateStore";

/**
 * Delay before the launch check. SIP registration happens immediately at
 * startup and matters far more than an update check, so we stay out of its way.
 */
const LAUNCH_CHECK_DELAY_MS = 8000;

/**
 * How often a running app checks again. A softphone is left open for days or
 * weeks at a time, and with only a launch check it never heard about a release
 * until someone happened to quit it. Checking during a call is harmless: the
 * prompt itself waits until no call is up.
 */
const PERIODIC_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

/** Module-scoped so the checks are scheduled once per app session, not per mount. */
let checksScheduled = false;

/**
 * Checks for an update a few seconds after launch, then every few hours.
 * Failures are silent — see `checkForUpdate` in the update store.
 */
export function useUpdateOnLaunch() {
  const checkForUpdate = useUpdateStore((s) => s.checkForUpdate);

  useEffect(() => {
    if (checksScheduled) return;
    checksScheduled = true;

    // Deliberately not cleared on unmount: the timers belong to the session,
    // and cancelling them would skip the checks entirely under StrictMode's
    // mount/unmount/remount in development.
    setTimeout(() => {
      void checkForUpdate(false);
    }, LAUNCH_CHECK_DELAY_MS);
    setInterval(() => {
      void checkForUpdate(false);
    }, PERIODIC_CHECK_INTERVAL_MS);
  }, [checkForUpdate]);
}
