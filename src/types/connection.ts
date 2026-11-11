/**
 * The backend's step-by-step picture of an account's registration, as sent
 * in `sip-connection` events and returned by `get_connection_report`. The
 * field names mirror `src-tauri/src/sip/connection.rs`.
 */

export type StepKind = "resolve" | "connect" | "register" | "authenticate" | "registered";

export type StepStatus = "ok" | "failed" | "running" | "pending" | "skipped";

export type FailureKind =
  | "dns"
  | "dns_timeout"
  | "unreachable"
  | "tls_untrusted"
  | "tls_name"
  | "tls_handshake"
  | "timeout"
  | "auth"
  | "forbidden_ip"
  | "server_error"
  | "other";

export type Outcome = "registering" | "registered" | "reconnecting" | "error" | "unregistered";

export interface ConnectionStep {
  kind: StepKind;
  status: StepStatus;
  /** What happened, in the OS's, TLS stack's or server's words. Shown verbatim. */
  detail: string | null;
  elapsedMs: number | null;
  failure: FailureKind | null;
}

export interface ConnectionReport {
  accountId: string;
  username: string;
  domain: string;
  transport: "udp" | "tcp" | "tls";
  target: { host: string; port: number; viaProxy: boolean };
  steps: ConnectionStep[];
  outcome: Outcome;
  /** The same text the account's registration state carries. */
  error: string | null;
  reconnectAttempt: number;
  registered: {
    registeredAtMs: number;
    expiresInS: number;
    lastRefreshMs: number;
    publicAddr: string | null;
  } | null;
  attemptStartedMs: number;
  attemptFinishedMs: number | null;
}

/**
 * What the frontend will do next about a failed account. The frontend owns
 * the retry timer, so this is kept here rather than in the backend's report.
 */
export type RetryPlan =
  | { kind: "scheduled"; at: number }
  | { kind: "stopped"; reason: "auth" | "gave_up" | "disabled" }
  | { kind: "none" };
