import type { DiagnosticLog } from "../components/diagnostics/types";
import { messageCategory, type MessageCategory } from "./sipMessage";

export interface LogFilter {
  /** Only this account's messages; every account when unset. */
  accountId?: string;
  /** The categories to show. App events count as "other". */
  categories: ReadonlySet<MessageCategory>;
  /** Case-insensitive text to look for in the summary, the message, the address or the Call-ID. */
  search: string;
  /** Only the messages of this dialog. */
  callId: string | null;
}

export function filterLogs(logs: ReadonlyArray<DiagnosticLog>, filter: LogFilter): DiagnosticLog[] {
  const needle = filter.search.trim().toLowerCase();
  return logs.filter((log) => {
    if (filter.accountId && log.accountId !== filter.accountId) return false;
    if (filter.callId && log.callId !== filter.callId) return false;
    const category: MessageCategory = log.direction === "app" ? "other" : messageCategory(log.raw);
    if (!filter.categories.has(category)) return false;
    if (needle) {
      const hay = `${log.summary}\n${log.raw}\n${log.remoteAddr}\n${log.callId ?? ""}`.toLowerCase();
      if (!hay.includes(needle)) return false;
    }
    return true;
  });
}
