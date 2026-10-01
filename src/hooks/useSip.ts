import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAppStore, getAccountWithPassword } from "../stores/appStore";
import type {
  ActiveCall,
  CallState,
  SipAccount,
  RegistrationState,
  PresenceState,
} from "../types/sip";
import { useRingtone } from "./useRingtone";
import { useRingback } from "./useRingback";
import { log } from "../utils/log";

interface RegistrationPayload {
  accountId: string;
  state: RegistrationState;
  error: string | null;
}

interface PresenceEntry {
  extension: string;
  state: string;
  displayName: string | null;
}

interface PresencePayload {
  accountId: string;
  entries: PresenceEntry[];
}

interface CallPayload {
  accountId: string;
  callId: string;
  state: string;
  remoteUri: string;
  remoteName: string | null;
  direction: string;
  sipCallId?: string;
  /** Set on the "ended" event when the call produced a recording. */
  recordingPath?: string;
}

interface RecordingPayload {
  accountId: string;
  callId: string;
  recording: boolean;
  path?: string;
}

/** Registers all enabled accounts with retry backoff (crucial for launch at boot). */
export async function registerAllEnabledAccounts() {
  const store = useAppStore.getState();
  const enabledAccounts = store.accounts.filter((a) => a.enabled);
  if (enabledAccounts.length === 0) return;

  log.info("[useAutoRegister] Enabled accounts to register:", enabledAccounts.map(a => ({
    id: a.id,
    username: a.username,
    transport: a.transport,
    port: a.port,
  })));

  for (const account of enabledAccounts) {
    log.info("[useAutoRegister] Registering account:", account.id);
    store.setAccountRegistrationState(account.id, "registering");

    let attempts = 0;
    const maxAttempts = 5;
    const retryDelays = [2000, 4000, 6000, 10000, 15000];

    const tryRegister = async () => {
      try {
        const accountWithPassword = await getAccountWithPassword(account);
        await sipRegister(accountWithPassword);
        log.info(`[useAutoRegister] Registration successful for ${account.id}`);
      } catch (e) {
        attempts++;
        log.error(`Auto-registration failed for ${account.id} (attempt ${attempts}/${maxAttempts}):`, e);
        store.setAccountRegistrationState(account.id, "error", String(e));
        if (attempts < maxAttempts) {
          const delay = retryDelays[attempts - 1] || 15000;
          log.info(`[useAutoRegister] Retrying registration in ${delay}ms...`);
          setTimeout(tryRegister, delay);
        }
      }
    };

    tryRegister();
  }

  if (store.activeAccountId) {
    await sipSetActiveAccount(store.activeAccountId).catch(() => {});
  }
}

/** Auto-registers ALL enabled accounts on app launch. */
export function useAutoRegister() {
  const setupComplete = useAppStore((s) => s.setupComplete);
  const hasRegistered = useRef(false);

  useEffect(() => {
    // Guard against React StrictMode double-execution
    if (hasRegistered.current) return;
    if (!setupComplete) return;

    hasRegistered.current = true;
    registerAllEnabledAccounts();
  }, [setupComplete]);
}

/** How long an ended call stays on screen before it is removed. */
const ENDED_DISPLAY_MS = 1200;

/**
 * Take a call out of service: file it in history, show it as ended briefly,
 * then remove it.
 *
 * The single place a call ends in the UI, whichever side hung up. Hangup used
 * to be handled here and again by each hangup button, so a local hangup filed
 * two history rows, and a button's delayed cleanup could wipe out a call
 * placed in the meantime. Calling this twice for one call is harmless.
 */
function finishCall(
  callId: string,
  details: { remoteUri?: string; remoteName?: string; recordingPath?: string; sipCallId?: string } = {},
) {
  const store = useAppStore.getState();
  const call = store.activeCalls.find((c) => c.id === callId);
  if (!call || call.state === "ended") return;

  const endTime = Date.now();
  store.addCallHistory({
    id: call.id,
    accountId: call.accountId,
    remoteUri: details.remoteUri || call.remoteUri,
    remoteName: details.remoteName ?? call.remoteName,
    direction: call.direction,
    startTime: call.startTime ?? endTime,
    duration: call.connectTime ? Math.floor((endTime - call.connectTime) / 1000) : 0,
    missed: !call.connectTime,
    // The backend attaches the path on every ended event, however the call
    // ended, because it is the one that finishes the recording.
    recordingPath: details.recordingPath ?? call.recordingPath,
    sipCallId: details.sipCallId ?? call.sipCallId,
  });
  store.updateCall(callId, { state: "ended", endTime, recording: false });
  setTimeout(() => useAppStore.getState().removeCall(callId), ENDED_DISPLAY_MS);
}

/** Whether any call is still in progress. */
function hasLiveCall(): boolean {
  return useAppStore.getState().activeCalls.some((c) => c.state !== "ended");
}

export function useSipEvents() {
  const activeCall = useAppStore((s) => s.activeCall);

  const isIncoming = activeCall?.state === "incoming";
  const isRinging = activeCall?.state === "ringing" || activeCall?.state === "dialing";
  useRingtone(isIncoming);
  useRingback(isRinging);

  // Track which accounts we've already subscribed presence for
  const subscribedAccounts = useRef<Set<string>>(new Set());

  // Registered once for the life of the app. Handlers read the store when an
  // event arrives rather than closing over it: this effect used to depend on
  // the active call, so every call-state change tore the listeners down and
  // re-registered them over IPC, and any event landing in that gap — often
  // the recording event that immediately follows "connected" — was lost.
  useEffect(() => {
    const unlistenReg = listen<RegistrationPayload>(
      "sip-registration",
      (event) => {
        const { accountId, state, error } = event.payload;
        log.info("[useSipEvents] Registration event received:", { accountId, state, error });
        useAppStore.getState().setAccountRegistrationState(accountId, state, error ?? undefined);

        // Auto-subscribe to presence for internal contacts after registration success
        if (state === "registered" && !subscribedAccounts.current.has(accountId)) {
          subscribedAccounts.current.add(accountId);
          autoSubscribePresence(useAppStore.getState().contacts);
        }
      },
    );

    // Listen for presence/BLF updates
    const unlistenPresence = listen<PresencePayload>(
      "sip-presence",
      (event) => {
        const { entries } = event.payload;
        if (entries && entries.length > 0) {
          useAppStore.getState().setPresenceBulk(
            entries.map((e) => ({
              extension: e.extension,
              state: mapPresenceState(e.state),
            })),
          );
        }
      },
    );

    const unlistenCall = listen<CallPayload>("sip-call", (event) => {
      const p = event.payload;
      const store = useAppStore.getState();

      if (p.state === "ended") {
        finishCall(p.callId, {
          remoteUri: p.remoteUri,
          remoteName: p.remoteName ?? undefined,
          recordingPath: p.recordingPath,
          sipCallId: p.sipCallId,
        });
        return;
      }

      if (p.state === "incoming") {
        if (hasLiveCall()) return;
        store.setActiveCall({
          id: p.callId,
          accountId: p.accountId,
          remoteUri: p.remoteUri,
          remoteName: p.remoteName ?? undefined,
          state: "incoming",
          direction: "inbound",
          startTime: Date.now(),
          muted: false,
          held: false,
          recording: false,
          sipCallId: p.sipCallId,
        });
        store.setCurrentView("dialer");
        return;
      }

      // Any call we know about, not only the one on screen: a held call in a
      // three-way still changes state.
      const call = store.activeCalls.find((c) => c.id === p.callId);
      if (!call || call.state === "ended") return;
      store.updateCall(p.callId, {
        state: p.state as CallState,
        connectTime:
          p.state === "connected" && !call.connectTime ? Date.now() : call.connectTime,
        sipCallId: p.sipCallId ?? call.sipCallId,
      });
    });

    // Auto-record is started by the backend, so the UI only learns a call is
    // being recorded from this event.
    const unlistenRecording = listen<RecordingPayload>("sip-recording", (event) => {
      const { callId, recording, path } = event.payload;
      log.info("[useSipEvents] Recording event received:", { callId, recording });
      useAppStore.getState().setCallRecording(callId, recording, path);
    });

    return () => {
      unlistenReg.then((fn_) => fn_());
      unlistenCall.then((fn_) => fn_());
      unlistenPresence.then((fn_) => fn_());
      unlistenRecording.then((fn_) => fn_());
    };
  }, []);
}

/**
 * Place an outbound call and show it immediately.
 *
 * The call's id is chosen here and handed to the backend, so the call is in
 * the store under its real id before the INVITE goes out. The backend used to
 * pick the id, and events it sent before `sipMakeCall` resolved named an id
 * the UI had never seen; a peer that answered instantly left the call stuck
 * on "dialing" and filed as missed.
 *
 * With `asAdditionalCall`, the call joins the existing ones (three-way
 * calling) instead of replacing the primary call's slot.
 */
export async function placeCall(
  call: { uri: string; accountId: string; remoteName?: string },
  { asAdditionalCall = false }: { asAdditionalCall?: boolean } = {},
): Promise<void> {
  const store = useAppStore.getState();
  const id = crypto.randomUUID();
  const newCall: ActiveCall = {
    id,
    accountId: call.accountId,
    remoteUri: call.uri,
    remoteName: call.remoteName,
    state: "dialing",
    direction: "outbound",
    startTime: Date.now(),
    muted: false,
    held: false,
    recording: false,
  };

  if (asAdditionalCall) {
    store.addCall(newCall);
    store.setPrimaryCall(id);
  } else {
    store.setActiveCall(newCall);
  }

  try {
    await (asAdditionalCall ? sipAddCall(call.uri, id) : sipMakeCall(call.uri, id));
  } catch (e) {
    log.error("[placeCall] Failed to place call:", e);
    useAppStore.getState().removeCall(id);
    throw e;
  }
}

/**
 * Hang up `call`. The backend saves any recording and reports the call ended,
 * which is what files it in history; this only does so itself when the
 * backend could not be reached, since no ended event will come then.
 */
export async function hangupCall(call: ActiveCall): Promise<void> {
  try {
    await sipHangup(call.id);
  } catch (e) {
    log.error("[hangupCall] Backend hangup failed, ending locally:", e);
    finishCall(call.id);
  }
}

export async function sipRegister(account: SipAccount): Promise<string> {
  log.info("[sipRegister] Registering account:", {
    id: account.id,
    username: account.username,
    domain: account.domain,
    transport: account.transport,
    port: account.port,
    hasPassword: !!account.password,
  });
  return invoke<string>("sip_register", {
    config: {
      id: account.id,
      displayName: account.displayName,
      username: account.username,
      domain: account.domain,
      password: account.password,
      transport: account.transport,
      port: account.port,
      registrar: account.registrar ?? null,
      outboundProxy: account.outboundProxy ?? null,
      authUsername: account.authUsername ?? null,
      authRealm: account.authRealm ?? null,
      enabled: account.enabled,
      // Every field the backend reads must be listed here. Omitting one does
      // not fall back to the stored account — it falls back to whatever serde
      // default the Rust struct declares, which silently discarded the user's
      // choice for both of these.
      autoRecord: account.autoRecord ?? false,
      srtpMode: account.srtpMode ?? null,
      codecs: account.codecs ?? null,
    },
  });
}

export async function sipUnregister(): Promise<void> {
  return invoke("sip_unregister");
}

export async function sipUnregisterAccount(accountId: string): Promise<void> {
  return invoke("sip_unregister_account", { accountId });
}

export async function sipSetActiveAccount(accountId: string): Promise<void> {
  return invoke("sip_set_active_account", { accountId });
}

export async function sipMakeCall(uri: string, callId?: string): Promise<string> {
  return invoke<string>("sip_make_call", { uri, callId });
}

export async function sipHangup(callId: string): Promise<void> {
  return invoke("sip_hangup", { callId });
}

export async function sipAnswer(callId: string): Promise<void> {
  return invoke("sip_answer", { callId });
}

export async function sipHold(callId: string, hold: boolean): Promise<void> {
  return invoke("sip_hold", { callId, hold });
}

export async function sipMute(callId: string, mute: boolean): Promise<void> {
  return invoke("sip_mute", { callId, mute });
}

export async function sipSendDtmf(
  callId: string,
  digit: string,
): Promise<void> {
  return invoke("sip_send_dtmf", { callId, digit });
}

export async function sipStartRecording(callId: string): Promise<string> {
  return invoke("sip_start_recording", { callId });
}

export async function sipStopRecording(callId: string): Promise<string | null> {
  return invoke("sip_stop_recording", { callId });
}

export async function sipIsRecording(callId: string): Promise<boolean> {
  return invoke("sip_is_recording", { callId });
}

// ── Conference Calling ─────────────────────────────────────────────────────

/** Start a second call (for three-way calling) - first call should be on hold */
export async function sipAddCall(uri: string, callId?: string): Promise<string> {
  return invoke<string>("sip_add_call", { uri, callId });
}

/** Merge multiple calls into a conference */
export async function sipConferenceMerge(callIds: string[]): Promise<string> {
  return invoke<string>("sip_conference_merge", { callIds });
}

/** Split a call from a conference */
export async function sipConferenceSplit(conferenceId: string, callId: string): Promise<void> {
  return invoke("sip_conference_split", { conferenceId, callId });
}

/** End a conference (hangs up all calls) */
export async function sipConferenceEnd(conferenceId: string): Promise<void> {
  return invoke("sip_conference_end", { conferenceId });
}

/** Swap between two calls (put one on hold, resume other) */
export async function sipSwapCalls(holdCallId: string, resumeCallId: string): Promise<void> {
  return invoke("sip_swap_calls", { holdCallId, resumeCallId });
}

export async function getDefaultRecordingsDir(): Promise<string> {
  return invoke("get_default_recordings_dir");
}

export async function openRecordingsFolder(customPath?: string): Promise<void> {
  return invoke("open_recordings_folder", { customPath });
}

export async function playRecording(path: string): Promise<void> {
  return invoke("play_recording", { path });
}

// ── System Contacts ─────────────────────────────────────────────────────────

export interface SystemContact {
  id: string;
  name: string;
  phone: string | null;
}

// ── Per-call diagnostics ─────────────────────────────────────────────────────

/** Export PCAP for a specific call by SIP Call-ID */
export async function exportCallPcap(
  sipCallId: string,
  path?: string,
): Promise<string> {
  return invoke<string>("export_call_pcap", { sipCallId, path });
}

/** Get SIP message trace for a specific call */
export async function getCallSipTrace(
  sipCallId: string,
): Promise<unknown[]> {
  return invoke<unknown[]>("get_call_sip_trace", { sipCallId });
}

export async function fetchSystemContacts(): Promise<SystemContact[]> {
  return invoke("fetch_system_contacts");
}

// ── Presence / BLF ──────────────────────────────────────────────────────────

/** Subscribe to presence for contact extensions */
export async function sipSubscribeBlf(extensions: string[]): Promise<string[]> {
  return invoke<string[]>("sip_subscribe_blf", { extensions });
}

/** Auto-subscribe to presence for all contacts that look like internal extensions */
async function autoSubscribePresence(contacts: Array<{ uri: string }>) {
  // Extract extensions from contacts
  const extensions = contacts
    .map((c) => {
      const withoutScheme = c.uri.replace(/^sips?:/, "");
      return withoutScheme.split("@")[0];
    })
    .filter((ext) => ext && /^\d{2,6}$/.test(ext)); // Only short numeric extensions (internal)

  if (extensions.length === 0) {
    log.info("[autoSubscribePresence] No internal extensions to subscribe to");
    return;
  }

  // Deduplicate
  const unique = [...new Set(extensions)];
  log.info("[autoSubscribePresence] Subscribing to presence for extensions:", unique);

  try {
    await sipSubscribeBlf(unique);
  } catch (e) {
    log.error("[autoSubscribePresence] Failed:", e);
  }
}

/** Map backend presence state strings to our PresenceState type */
function mapPresenceState(state: string): PresenceState {
  switch (state) {
    case "available":
      return "available";
    case "busy":
      return "busy";
    case "away":
      return "away";
    case "onThePhone":
      return "onThePhone";
    case "ringing":
      return "ringing";
    case "doNotDisturb":
      return "doNotDisturb";
    case "unknown":
      return "unknown";
    default:
      return "offline";
  }
}
