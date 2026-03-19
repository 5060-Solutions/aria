import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAppStore, getAccountWithPassword } from "../stores/appStore";
import type { SipAccount, RegistrationState, PresenceState } from "../types/sip";
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
}

/** Auto-registers ALL enabled accounts on app launch. */
export function useAutoRegister() {
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const setupComplete = useAppStore((s) => s.setupComplete);
  const setAccountRegistrationState = useAppStore(
    (s) => s.setAccountRegistrationState
  );
  const hasRegistered = useRef(false);

  useEffect(() => {
    // Guard against React StrictMode double-execution
    if (hasRegistered.current) return;
    if (!setupComplete) return;

    const enabledAccounts = accounts.filter((a) => a.enabled);
    if (enabledAccounts.length === 0) return;

    hasRegistered.current = true;

    // Register all enabled accounts concurrently
    const registerAll = async () => {
      log.info("[useAutoRegister] Enabled accounts to register:", enabledAccounts.map(a => ({
        id: a.id,
        username: a.username,
        transport: a.transport,
        port: a.port,
      })));

      for (const account of enabledAccounts) {
        log.info("[useAutoRegister] Registering account:", account.id, "transport:", account.transport, "port:", account.port);
        setAccountRegistrationState(account.id, "registering");
        try {
          const accountWithPassword = await getAccountWithPassword(account);
          log.info("[useAutoRegister] Account with password:", {
            id: accountWithPassword.id,
            username: accountWithPassword.username,
            transport: accountWithPassword.transport,
            port: accountWithPassword.port,
          });
          await sipRegister(accountWithPassword);
        } catch (e) {
          log.error(`Auto-registration failed for ${account.id}:`, e);
          setAccountRegistrationState(account.id, "error", String(e));
        }
      }

      // Set active account in backend if we have one
      if (activeAccountId) {
        await sipSetActiveAccount(activeAccountId).catch(() => {});
      }
    };

    registerAll();
    // Only run once on mount — accounts/activeAccountId are initial values
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}

export function useSipEvents() {
  const setAccountRegistrationState = useAppStore((s) => s.setAccountRegistrationState);
  const setActiveCall = useAppStore((s) => s.setActiveCall);
  const activeCall = useAppStore((s) => s.activeCall);
  const addCallHistory = useAppStore((s) => s.addCallHistory);
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const setPresenceBulk = useAppStore((s) => s.setPresenceBulk);

  const isIncoming = activeCall?.state === "incoming";
  const isRinging = activeCall?.state === "ringing" || activeCall?.state === "dialing";
  useRingtone(isIncoming);
  useRingback(isRinging);

  // Track which accounts we've already subscribed presence for
  const subscribedAccounts = useRef<Set<string>>(new Set());

  useEffect(() => {
    const unlistenReg = listen<RegistrationPayload>(
      "sip-registration",
      (event) => {
        const { accountId, state, error } = event.payload;
        log.info("[useSipEvents] Registration event received:", { accountId, state, error });
        setAccountRegistrationState(accountId, state, error ?? undefined);

        // Auto-subscribe to presence for internal contacts after registration success
        if (state === "registered" && !subscribedAccounts.current.has(accountId)) {
          subscribedAccounts.current.add(accountId);
          // Read contacts from store at the time of registration (not stale closure)
          const currentContacts = useAppStore.getState().contacts;
          autoSubscribePresence(currentContacts);
        }
      },
    );

    // Listen for presence/BLF updates
    const unlistenPresence = listen<PresencePayload>(
      "sip-presence",
      (event) => {
        const { entries } = event.payload;
        if (entries && entries.length > 0) {
          setPresenceBulk(
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

      if (p.state === "ended") {
        if (activeCall && activeCall.id === p.callId) {
          addCallHistory({
            id: p.callId,
            accountId: activeCall.accountId,
            remoteUri: p.remoteUri || activeCall.remoteUri,
            remoteName: p.remoteName ?? activeCall.remoteName,
            direction: activeCall.direction,
            startTime: activeCall.startTime ?? Date.now(),
            duration: activeCall.connectTime
              ? Math.floor((Date.now() - activeCall.connectTime) / 1000)
              : 0,
            missed: !activeCall.connectTime,
            sipCallId: p.sipCallId ?? activeCall.sipCallId,
          });
        }
        setActiveCall(null);
        return;
      }

      if (p.state === "incoming" && !activeCall) {
        setActiveCall({
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
        setCurrentView("dialer");
        return;
      }

      // Update existing call state
      if (activeCall && activeCall.id === p.callId) {
        setActiveCall({
          ...activeCall,
          state: p.state as typeof activeCall.state,
          connectTime:
            p.state === "connected" && !activeCall.connectTime
              ? Date.now()
              : activeCall.connectTime,
          sipCallId: p.sipCallId ?? activeCall.sipCallId,
        });
      }
    });

    return () => {
      unlistenReg.then((fn_) => fn_());
      unlistenCall.then((fn_) => fn_());
      unlistenPresence.then((fn_) => fn_());
    };
  }, [activeCall, setAccountRegistrationState, setActiveCall, addCallHistory, setCurrentView, setPresenceBulk]);
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
      enabled: account.enabled,
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

export async function sipMakeCall(uri: string): Promise<string> {
  return invoke<string>("sip_make_call", { uri });
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
export async function sipAddCall(uri: string): Promise<string> {
  return invoke<string>("sip_add_call", { uri });
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
