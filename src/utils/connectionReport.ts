import type { ConnectionReport, ConnectionStep, RetryPlan, StepKind } from "../types/connection";

/** The shape of i18next's `t` that these helpers need. */
export type Translate = (key: string, options?: Record<string, unknown>) => string;

/** The step that broke this attempt, if any. */
export function failedStep(report: ConnectionReport): ConnectionStep | null {
  return report.steps.find((s) => s.status === "failed") ?? null;
}

export function stepLabelKey(kind: StepKind): string {
  return `connection.steps.${kind}`;
}

/** `user@domain · TLS · host:port`, with `proxy ` in front of a proxy. */
export function accountLine(report: ConnectionReport): string {
  const { host, port, viaProxy } = report.target;
  const target = `${viaProxy ? "proxy " : ""}${host}:${port}`;
  return `${report.username}@${report.domain} · ${report.transport.toUpperCase()} · ${target}`;
}

/**
 * What will happen next about this account, or null when there is nothing to
 * say (registered, or no retry of any kind).
 */
export function retryLine(
  report: ConnectionReport,
  plan: RetryPlan,
  now: number,
  t: Translate,
): string | null {
  if (report.outcome === "registered") return null;
  if (report.outcome === "reconnecting") {
    return t("connection.retry.reconnecting", { attempt: report.reconnectAttempt });
  }
  switch (plan.kind) {
    case "scheduled":
      return t("connection.retry.in", { seconds: Math.max(0, Math.ceil((plan.at - now) / 1000)) });
    case "stopped":
      switch (plan.reason) {
        case "auth":
          return t("connection.retry.stoppedAuth");
        case "gave_up":
          return t("connection.retry.onWake");
        case "disabled":
          return t("connection.retry.disabled");
      }
      return null;
    case "none":
      return null;
  }
}

function clock(ms: number): string {
  const d = new Date(ms);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

function span(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const m = Math.floor(s / 60);
  return m > 0 ? `${m} min ${s % 60} s` : `${s} s`;
}

/** "expires in 9 min 40 s · last refresh 10:42:18", or null when not registered. */
export function registeredLine(report: ConnectionReport, now: number, t: Translate): string | null {
  const info = report.registered;
  if (report.outcome !== "registered" || !info) return null;
  const expiresAt = info.registeredAtMs + info.expiresInS * 1000;
  return t("connection.registeredDetail", {
    expires: span((expiresAt - now) / 1000),
    at: clock(info.lastRefreshMs),
  });
}

/** The panel as plain text, for pasting into a ticket. */
export function copyDetails(
  report: ConnectionReport,
  plan: RetryPlan,
  meta: { appVersion: string; os: string; now: number },
  t: Translate,
): string {
  const lines = [`Aria ${meta.appVersion} on ${meta.os}`, accountLine(report)];
  for (const step of report.steps) {
    let line = `[${step.status}] ${t(stepLabelKey(step.kind))}`;
    if (step.detail) line += ` — ${step.detail}`;
    if (step.elapsedMs !== null) line += ` (${step.elapsedMs} ms)`;
    lines.push(line);
  }
  if (report.error) lines.push(`Error: ${report.error}`);
  const retry = retryLine(report, plan, meta.now, t);
  if (retry) lines.push(retry);
  const registered = registeredLine(report, meta.now, t);
  if (registered) lines.push(registered);
  lines.push(`Captured ${new Date(meta.now).toISOString()}`);
  return lines.join("\n");
}
