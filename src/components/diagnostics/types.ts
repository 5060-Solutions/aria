/** Types shared by the Diagnostics window; they mirror the backend's JSON. */

export interface DiagnosticLog {
  timestamp: number;
  accountId: string;
  /** "app" is something Aria did (e.g. an MCP call-control action), not SIP. */
  direction: "sent" | "received" | "app";
  remoteAddr: string;
  summary: string;
  raw: string;
  callId?: string;
}

/** A key that follows a message rather than its position in the list. */
export function logKey(log: DiagnosticLog): string {
  return `${log.timestamp}-${log.accountId}-${log.direction}-${log.remoteAddr}-${log.raw.length}`;
}

export interface RtpStats {
  packetsSent: number;
  packetsReceived: number;
  bytesSent: number;
  bytesReceived: number;
  packetsLost: number;
  jitterMs: number;
  codecName: string;
}

export interface CallStatusInfo {
  id: string;
  accountId: string;
  remoteUri: string;
  state: string;
  direction: string;
  durationSecs: number | null;
  codec: string | null;
  rtpStats: RtpStats | null;
}

export interface AccountStatus {
  accountId: string;
  username: string;
  domain: string;
  registrationState: string;
  registrationError: string | null;
  serverAddress: string | null;
  transportType: string | null;
  localAddress: string | null;
  publicAddress: string | null;
  uptimeSecs: number | null;
  activeCalls: CallStatusInfo[];
}

export interface SystemStatus {
  accounts: AccountStatus[];
  latencyMs: number | null;
  totalActiveCalls: number;
}

/** `get_app_info`: what this build is. */
export interface AppInfo {
  version: string;
  os: string;
  arch: string;
  profile: string;
  transcription: boolean;
}
