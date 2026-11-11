import type { ConnectionReport, FailureKind } from "../types/connection";
import { failedStep } from "./connectionReport";

/** Failure kinds that have a "what this usually means" hint. */
const HINTED: ReadonlySet<FailureKind> = new Set<FailureKind>([
  "dns",
  "dns_timeout",
  "unreachable",
  "tls_untrusted",
  "tls_name",
  "tls_handshake",
  "timeout",
  "auth",
  "forbidden_ip",
  "server_error",
]);

/**
 * The i18n key of the hint for the step that failed, or null when nothing
 * failed or the backend could not say what kind of failure it was. A wrong
 * hint is worse than none, so an unclassified failure gets none.
 */
export function hintKey(report: ConnectionReport): string | null {
  const failed = failedStep(report);
  if (!failed?.failure || !HINTED.has(failed.failure)) return null;
  return `connection.hints.${failed.failure}`;
}
