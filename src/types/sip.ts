/** `auto` lets DNS SRV pick TLS, TCP or UDP, in that order of preference. */
export type TransportType = "udp" | "tcp" | "tls" | "auto";

/** SRTP mode for media encryption */
export type SrtpMode = "disabled" | "sdes";

/** Stored accounts may still say "dtls", a mode that was never implemented and sent plain RTP. */
export function normalizeSrtpMode(mode: string | undefined): SrtpMode {
  return mode === "sdes" ? "sdes" : "disabled";
}

/**
 * Audio codec types supported by Aria. The list mirrors `SUPPORTED_CODECS`
 * in `src-tauri/src/sip/builder.rs`: a codec named here is offered and
 * answered; anything else saved in an account is dropped on load.
 */
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
  { codec: "pcmu", enabled: true, priority: 2 },
  { codec: "pcma", enabled: true, priority: 3 },
  { codec: "g729", enabled: true, priority: 4 },
];

/** Codec display information */
export const CODEC_INFO: Record<CodecType, { name: string; bitrate: string; description: string }> = {
  pcmu: { name: "G.711 μ-law", bitrate: "64 kbps", description: "Universal compatibility" },
  pcma: { name: "G.711 A-law", bitrate: "64 kbps", description: "Common in Europe" },
  g729: { name: "G.729", bitrate: "8 kbps", description: "Low bandwidth" },
  opus: { name: "Opus", bitrate: "~32 kbps", description: "Modern, high quality" },
};

/**
 * Saved codec preferences as the softphone can use them: codecs it does not
 * carry are dropped, codecs added since the account was saved are appended
 * after the user's own order, enabled as the defaults have them, and the
 * list is renumbered. The defaults stand in when nothing usable was saved.
 */
export function normalizeCodecs(
  saved: ReadonlyArray<{ codec: string; enabled: boolean; priority: number }> | null | undefined,
): CodecConfig[] {
  const usable = (saved ?? [])
    .filter((c): c is CodecConfig => c.codec in CODEC_INFO)
    .slice()
    .sort((a, b) => a.priority - b.priority);
  if (usable.length === 0) return DEFAULT_CODECS.map((c) => ({ ...c }));
  const missing = DEFAULT_CODECS.filter((d) => !usable.some((c) => c.codec === d.codec));
  return [...usable, ...missing].map((c, index) => ({ codec: c.codec, enabled: c.enabled, priority: index + 1 }));
}

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
  /** What to dial to reach voicemail (e.g. *97). From provisioning or Settings. */
  voicemailNumber?: string;
  /**
   * Name the TLS certificate is checked against when it is neither the
   * registrar nor the domain (a setup link's outbound proxy host).
   */
  tlsServerName?: string;
  /**
   * `host[:port]` of the setup link that created this account; unset for
   * accounts set up by hand. Only that host's links may update it.
   */
  provisionedFrom?: string;
  /**
   * The Aria PBX REST API (`https://host[:port]`) for visual voicemail. Set
   * from the setup link's origin, or by hand in Advanced; unset means the
   * voicemail button dials the mailbox instead.
   */
  pbxApiUrl?: string;
  /** Whether to automatically add country code to dialed numbers */
  autoCountryCode?: boolean;
  /** Prefix to remove from dialed numbers before calling (e.g. "0" or "98") */
  stripPrefix?: string;
  /** Prefix to prepend to dialed numbers before calling (e.g. "9" or "0") */
  prependPrefix?: string;
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
  voicemailNumber?: string;
  autoCountryCode?: boolean;
  stripPrefix?: string;
  prependPrefix?: string;
  tlsServerName?: string;
  provisionedFrom?: string;
  pbxApiUrl?: string;
}

/** Message-waiting state for one account, from the PBX's NOTIFY. */
export interface VoicemailStatus {
  hasMessages: boolean;
  newMessages: number;
  oldMessages: number;
}

/** Progress of a transfer the user started, from the backend's REFER handling. */
export interface TransferStatus {
  callId: string;
  /** SIP status: 100 while in progress, 2xx done, 4xx and up failed. */
  status: number;
  message: string;
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
  /**
   * Whether this call's media is actually encrypted, as negotiated.
   *
   * Not the account's srtpMode: that is a preference, and the indicator used
   * to read it directly — so it showed a padlock whenever SRTP was configured,
   * including for the since-removed DTLS option and whenever the far end answered
   * without a crypto line.
   */
  srtpActive?: boolean;
  /**
   * Whether the call's signalling ran over TLS, reported with srtpActive.
   * SDES sends the SRTP key in the signalling, so without TLS the media is
   * encrypted but its key is readable on the path.
   */
  signalingTls?: boolean;
  /** SIP Call-ID header (for linking to PCAP/diagnostic traces) */
  sipCallId?: string;
  /**
   * Whether early media is active on this call (180/183 with SDP).
   * When true, in-band network audio from the carrier is playing,
   * so local ringback tone generation must be suppressed.
   */
  hasEarlyMedia?: boolean;
  /**
   * Set on an incoming call that is the PBX moving a call to this device
   * (`X-Aria-Move`). `moveAuto` means this app asked for it: it is answered
   * without ringing. Otherwise the call rings, labelled as a move.
   */
  moveId?: string;
  moveAuto?: boolean;
  /** For a moved call: when it originally started, so the timer carries over once it connects. */
  moveStartTime?: number;
  /** The device a ringing move comes from, when known. */
  moveFrom?: string;
  /** The device the PBX moved this call to, once it has (the call then ends). */
  movedTo?: string;
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
  /** Presence status from BLF/presence subscription */
  status?: string;
}

/** Presence state from SIP presence/BLF notifications */
export type PresenceState =
  | "available"
  | "busy"
  | "away"
  | "onThePhone"
  | "ringing"
  | "doNotDisturb"
  | "unknown"
  | "offline";

export interface AudioDevice {
  id: string;
  name: string;
  kind: "input" | "output";
  isDefault: boolean;
}

/** What the user asked the audio processor to do. */
export interface VoiceProcessingSettings {
  /** Subtract the speaker signal from the microphone signal. */
  echoCancellation: boolean;
  /** Attenuate steady background noise. */
  noiseSuppression: boolean;
  /** Normalise a microphone that is too quiet or too loud. */
  gainControl: boolean;
}

/** What the backend reports back, including whether any of it is real. */
export interface VoiceProcessingStatus {
  settings: VoiceProcessingSettings;
  /**
   * Whether this build can do any of it. False means the switches must be
   * disabled rather than shown as on.
   */
  available: boolean;
  /**
   * Whether it is running on the call in progress; null when there is no call.
   * Can be false while `settings.echoCancellation` is true — most often
   * because the microphone and speaker run at different sample rates.
   */
  activeOnCall: boolean | null;
}
