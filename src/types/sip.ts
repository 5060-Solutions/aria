export type TransportType = "udp" | "tcp" | "tls";

/** SRTP mode for media encryption */
export type SrtpMode = "disabled" | "sdes" | "dtls";

/** Audio codec types supported by Aria */
export type CodecType = "pcmu" | "pcma" | "g729" | "opus";

/** Codec configuration with priority ordering */
export interface CodecConfig {
  /** Codec type identifier */
  codec: CodecType;
  /** Whether this codec is enabled */
  enabled: boolean;
  /** Priority (lower = higher priority, 1 is highest) */
  priority: number;
}

/** Default codec configuration - ordered by common preference */
export const DEFAULT_CODECS: CodecConfig[] = [
  { codec: "opus", enabled: true, priority: 1 },
  { codec: "g729", enabled: true, priority: 2 },
  { codec: "pcmu", enabled: true, priority: 3 },
  { codec: "pcma", enabled: true, priority: 4 },
];

/** Codec display information */
export const CODEC_INFO: Record<CodecType, { name: string; bitrate: string; description: string }> = {
  pcmu: { name: "G.711 μ-law", bitrate: "64 kbps", description: "Universal compatibility" },
  pcma: { name: "G.711 A-law", bitrate: "64 kbps", description: "Common in Europe" },
  g729: { name: "G.729A", bitrate: "8 kbps", description: "Low bandwidth, widely supported" },
  opus: { name: "Opus", bitrate: "~32 kbps", description: "Modern, high quality" },
};

export type RegistrationState =
  | "unregistered"
  | "registering"
  | "registered"
  | "reconnecting"
  | "error";

export type CallState =
  | "idle"
  | "dialing"
  | "ringing"
  | "incoming"
  | "connected"
  | "held"
  | "conferenced"
  | "ended";

/** Conference call state */
export interface Conference {
  id: string;
  callIds: string[];
  startTime: number;
}

export type CallDirection = "inbound" | "outbound";

export interface SipAccount {
  id: string;
  displayName: string;
  username: string;
  domain: string;
  password: string;
  transport: TransportType;
  port: number;
  registrar?: string;
  outboundProxy?: string;
  authUsername?: string;
  /** Override realm used for authentication */
  authRealm?: string;
  enabled: boolean;
  autoRecord?: boolean;
  /** SRTP mode - disabled, optional (offer but accept plain), or required */
  srtpMode?: SrtpMode;
  /** Codec configuration - if not set, uses DEFAULT_CODECS */
  codecs?: CodecConfig[];
}

/** Account data stored in localStorage (without password) */
export interface StoredAccount {
  id: string;
  displayName: string;
  username: string;
  domain: string;
  transport: TransportType;
  port: number;
  registrar?: string;
  outboundProxy?: string;
  authUsername?: string;
  authRealm?: string;
  enabled: boolean;
  autoRecord?: boolean;
  srtpMode?: SrtpMode;
  codecs?: CodecConfig[];
}

export interface AccountState {
  accountId: string;
  registrationState: RegistrationState;
  registrationError: string | null;
}

export interface ActiveCall {
  id: string;
  accountId: string;
  remoteUri: string;
  remoteName?: string;
  state: CallState;
  direction: CallDirection;
  startTime?: number;
  connectTime?: number;
  endTime?: number;
  muted: boolean;
  held: boolean;
  recording: boolean;
  recordingPath?: string;
  /** Conference ID if this call is part of a conference */
  conferenceId?: string;
  /** SIP Call-ID header (for linking to PCAP/diagnostic traces) */
  sipCallId?: string;
}

export interface CallHistoryEntry {
  id: string;
  accountId: string;
  remoteUri: string;
  remoteName?: string;
  direction: CallDirection;
  startTime: number;
  duration: number;
  missed: boolean;
  recordingPath?: string;
  /** SIP Call-ID header (for retrieving per-call PCAP from diagnostics) */
  sipCallId?: string;
}

export type ContactSource = "local" | "google" | "system";

export interface Contact {
  id: string;
  name: string;
  uri: string;
  phone?: string;
  avatar?: string;
  favorite: boolean;
  source: ContactSource;
  sourceId?: string;
}

export interface AudioDevice {
  id: string;
  name: string;
  kind: "input" | "output";
  isDefault: boolean;
}
