import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { log } from "./log";

/**
 * A desktop notification for a new inbound text. The permission is asked for
 * the first time one is shown, not at launch; a refusal is remembered by the
 * OS and the notification is skipped silently (the badge still counts it).
 */
export async function notifyText(title: string, body: string): Promise<void> {
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (granted) sendNotification({ title, body });
  } catch (e) {
    log.warn("[messaging] notification failed:", e);
  }
}
