/**
 * Time formatting for the Diagnostics window. SIP timing is a millisecond
 * business (retransmissions at 500 ms, a 401 that comes back in 40 ms), so
 * every stamp carries milliseconds, and the gap to the previous message is
 * shown as well: the gap is usually the thing being diagnosed.
 */

const timeFormatters = new Map<string, Intl.DateTimeFormat>();

function timeFormatter(locale: string, timeZone?: string): Intl.DateTimeFormat {
  const key = `${locale}|${timeZone ?? ""}`;
  let f = timeFormatters.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat(locale, {
      hour12: false,
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      timeZone,
    });
    timeFormatters.set(key, f);
  }
  return f;
}

function pad3(n: number): string {
  return n.toString().padStart(3, "0");
}

/** `HH:MM:SS.mmm`, 24-hour, in `locale`'s digits. */
export function formatLogTime(timestampMs: number, locale: string, timeZone?: string): string {
  const date = new Date(timestampMs);
  const parts = timeFormatter(locale, timeZone).formatToParts(date);
  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? "00";
  // Some locales render midnight as "24"; the log reads it as a clock.
  const hour = get("hour") === "24" ? "00" : get("hour");
  const ms = timeZone ? date.getUTCMilliseconds() : date.getMilliseconds();
  return `${hour}:${get("minute")}:${get("second")}.${pad3(ms)}`;
}

/** `YYYY-MM-DD HH:MM:SS.mmm ZONE`, for a tooltip or a copied line. */
export function formatLogTimestamp(timestampMs: number, timeZone?: string): string {
  const date = new Date(timestampMs);
  const parts = new Intl.DateTimeFormat("en-CA", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    timeZoneName: "short",
    timeZone,
  }).formatToParts(date);
  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? "";
  const hour = get("hour") === "24" ? "00" : get("hour");
  const ms = timeZone ? date.getUTCMilliseconds() : date.getMilliseconds();
  return `${get("year")}-${get("month")}-${get("day")} ${hour}:${get("minute")}:${get("second")}.${pad3(ms)} ${get("timeZoneName")}`;
}

/**
 * How long after the previous message this one arrived: `+123 ms`,
 * `+1.23 s`, `+1:01 min`. Empty for the first message.
 */
export function formatDelta(deltaMs: number | null): string {
  if (deltaMs === null) return "";
  const ms = Math.max(0, Math.round(deltaMs));
  if (ms < 1000) return `+${ms} ms`;
  if (ms < 10_000) return `+${(ms / 1000).toFixed(2)} s`;
  if (ms < 60_000) return `+${(ms / 1000).toFixed(1)} s`;
  const totalSecs = Math.round(ms / 1000);
  const minutes = Math.floor(totalSecs / 60);
  const seconds = totalSecs % 60;
  return `+${minutes}:${seconds.toString().padStart(2, "0")} min`;
}

const dayFormatters = new Map<string, Intl.DateTimeFormat>();

function dayKey(timestampMs: number, timeZone?: string): string {
  const d = new Date(timestampMs);
  if (timeZone) {
    return new Intl.DateTimeFormat("en-CA", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).format(d);
  }
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

/**
 * The date to print above a message when it is the first one or the day
 * changed since the previous message; `null` otherwise.
 */
export function daySeparator(
  previousMs: number | null,
  timestampMs: number,
  locale: string,
  timeZone?: string,
): string | null {
  if (previousMs !== null && dayKey(previousMs, timeZone) === dayKey(timestampMs, timeZone)) return null;
  const key = `${locale}|${timeZone ?? ""}`;
  let f = dayFormatters.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat(locale, { year: "numeric", month: "short", day: "numeric", timeZone });
    dayFormatters.set(key, f);
  }
  return f.format(new Date(timestampMs));
}
