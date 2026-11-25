mod ai;
mod audio_test;
mod commands;
mod diagnostics_window;
mod mcp;
#[cfg(target_os = "macos")]
mod menu;
mod provision;
mod sip;
mod system_contacts;
mod voicemail;

use tauri::{Emitter, Listener, Manager};

/// The app's bundle identifier, matching `identifier` in tauri.conf.json.
///
/// Code that can reach an `AppHandle` should use `app.path().app_data_dir()`
/// instead of this — Tauri resolves the identifier and the per-platform data
/// root together. This exists for the few paths that are computed from static
/// functions with no handle available, which previously hardcoded their own
/// copy of the identifier and got it wrong. `bundle_identifier_matches_config`
/// pins it to tauri.conf.json so the two cannot drift apart again.
///
/// Deliberately not cfg-gated to its callers (`ai`, `dev-insecure`), which are
/// both off in a default build: the drift test is worth running on every
/// `cargo test`, not only on feature-enabled ones.
#[allow(dead_code)]
pub(crate) const BUNDLE_IDENTIFIER: &str = "com.5060solutions.aria";

/// The app's data directory, equivalent to Tauri's `app_data_dir()`.
///
/// Only for callers with no `AppHandle`; prefer `app.path().app_data_dir()`.
/// Unused in a default build — see `BUNDLE_IDENTIFIER`.
#[allow(dead_code)]
pub(crate) fn app_data_root() -> Option<std::path::PathBuf> {
    dirs::data_dir().map(|d| d.join(BUNDLE_IDENTIFIER))
}

/// Bring the window back for a ringing call: a minimized or hidden Aria used
/// to ring with nothing on screen to answer it. It asks for attention (Dock
/// bounce, taskbar flash) rather than taking keyboard focus from whatever the
/// user is typing into.
fn surface_for_incoming_call(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    if !window.is_visible().unwrap_or(true) {
        let _ = window.show();
    }
    if !window.is_focused().unwrap_or(false) {
        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
    }
}

/// Whether a read from the default credential store says the store itself is
/// unreachable, as opposed to the entry being missing.
#[cfg(any(target_os = "linux", test))]
fn credential_store_unavailable(probe: &keyring::Result<String>) -> bool {
    matches!(probe, Err(keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)))
}

/// Fall back to the kernel keyring alone when there is no Secret Service.
///
/// The Linux store (`linux-native-sync-persistent`) writes keyutils and then
/// the Secret Service, and undoes the keyutils write when the second fails;
/// a read that misses keyutils asks the Secret Service. A session with no
/// Secret Service provider (i3 or sway without gnome-keyring, WSLg,
/// containers) could therefore neither save the SIP password nor give the
/// MCP server a token. There the store is switched to keyutils only, which
/// keeps credentials for the login session but not across a reboot — what
/// those sessions had before the persistent store.
///
/// Must run before anything creates a keyring entry.
#[cfg(target_os = "linux")]
fn use_session_keyring_without_secret_service() {
    let probe = keyring::Entry::new("aria-softphone", "keyring-probe").and_then(|e| e.get_password());
    if credential_store_unavailable(&probe) {
        log::warn!(
            "No Secret Service is available ({}); saved credentials are kept in the kernel keyring \
             and will not survive a reboot",
            probe.err().map(|e| e.to_string()).unwrap_or_default()
        );
        keyring::set_default_credential_builder(keyring::keyutils::default_credential_builder());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    #[cfg(target_os = "linux")]
    use_session_keyring_without_secret_service();

    // Create manager and take receiver before moving into Tauri. This runs on
    // the bare main thread with no Tokio runtime: construction spawns nothing,
    // and the manager's background loops start in setup() below.
    let (manager, event_rx) = sip::SipManager::new_with_receiver();
    let audio_test_manager = audio_test::AudioTestManager::new();

    let builder = tauri::Builder::default();

    // macOS only: the app menu gets About, Check for Updates… and Settings…,
    // handled in the webview. Windows and Linux have no menu bar to add them
    // to; see menu.rs.
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu::build).on_menu_event(menu::handle_event);

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        // Signature-verified updates. The frontend asks before installing —
        // never silently restart a softphone, which could drop a live call.
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Lets the update flow relaunch after installing, so the user gets one
        // click instead of "now quit and reopen the app yourself".
        .plugin(tauri_plugin_process::init())
        // On Linux this plugin claims a D-Bus well-known name, which by default
        // is derived from the bundle identifier. D-Bus requires that no
        // dot-separated element begins with a digit, and ours is
        // `com.5060solutions.aria` — the `5060solutions` element is invalid, so
        // the plugin's `.unwrap()` on the parsed name panicked at startup and
        // neither the .deb nor the AppImage could launch. Supply a valid name
        // explicitly rather than renaming the bundle, which would change the
        // app's identity and invalidate macOS/Windows signing.
        .plugin(
            tauri_plugin_single_instance::Builder::new()
                // The plugin has already forwarded any deep link in argv to
                // the deep-link plugin (the `deep-link` feature). Launching
                // Aria again is also how people ask for the window back.
                .callback(|app, _argv, _cwd| {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                })
                .dbus_id("com.solutions5060.aria")
                .build(),
        )
        .manage(manager)
        .manage(audio_test_manager)
        .manage(voicemail::VoicemailState::new())
        .setup(move |app| {
            // Start the manager's sweeper and timers. setup() itself runs on
            // the main thread from the event loop's Ready handler, with no
            // Tokio runtime in context (Tauri does not enter one), so the
            // start goes through Tauri's runtime: inside this task `tokio::spawn`
            // has a reactor. A bare call here would panic like v0.5.0 did.
            let sip_manager = app.state::<sip::SipManager>().inner().clone();
            tauri::async_runtime::spawn(async move {
                sip_manager.start_background_tasks();
            });

            // Set up event forwarding from SIP manager to frontend ONCE at startup
            let app_handle = app.handle().clone();
            
            // Spawn the event forwarding task with the receiver we took earlier
            let mut rx = event_rx;
            let call_manager = app.state::<sip::SipManager>().inner().clone();
            tauri::async_runtime::spawn(async move {
                // scripts/smoke-launch.sh waits for this exact line to decide
                // the app came up: it is logged unconditionally at info once
                // the app is built and its event loop is running. Change the
                // text there if you change it here.
                log::info!("SIP event forwarding started");
                while let Some(event) = rx.recv().await {
                        match event {
                            sip::SipEvent::RegistrationChanged(reg_event) => {
                                let payload = serde_json::json!({
                                    "accountId": reg_event.account_id,
                                    "state": reg_event.state,
                                    "error": reg_event.error,
                                });
                                log::info!("Emitting sip-registration event: {:?}", payload);
                                let _ = app_handle.emit("sip-registration", payload);
                            }
                            sip::SipEvent::ConnectionReport(report) => {
                                let payload = serde_json::json!({
                                    "accountId": report.account_id,
                                    "report": report,
                                });
                                let _ = app_handle.emit("sip-connection", payload);
                            }
                            sip::SipEvent::CallStateChanged(call_event) => {
                                if call_event.state == "incoming" {
                                    surface_for_incoming_call(&app_handle);
                                }
                                // The padlock needs the signalling transport as
                                // well as SRTP: an SDES key sent over UDP/TCP is
                                // readable by anyone on the path.
                                let mut payload = serde_json::to_value(&call_event).unwrap_or_default();
                                if call_event.srtp_active.is_some() {
                                    let tls = call_manager.signaling_is_tls(&call_event.account_id).await;
                                    payload["signalingTls"] = serde_json::Value::Bool(tls);
                                }
                                let _ = app_handle.emit("sip-call", payload);
                            }
                            sip::SipEvent::DiagnosticMessage(log_entry) => {
                                let _ = app_handle.emit("sip-diagnostic", log_entry);
                            }
                            sip::SipEvent::TransferProgress(transfer_event) => {
                                let _ = app_handle.emit("sip-transfer", transfer_event);
                            }
                            sip::SipEvent::PresenceChanged(account_id, blf_entries) => {
                                let payload = serde_json::json!({
                                    "accountId": account_id,
                                    "entries": blf_entries,
                                });
                                let _ = app_handle.emit("sip-presence", payload);
                            }
                            sip::SipEvent::ConferenceCreated { conference_id, call_ids } => {
                                let payload = serde_json::json!({
                                    "conferenceId": conference_id,
                                    "callIds": call_ids,
                                });
                                let _ = app_handle.emit("sip-conference-created", payload);
                            }
                            sip::SipEvent::ConferenceSplit { conference_id, call_id } => {
                                let payload = serde_json::json!({
                                    "conferenceId": conference_id,
                                    "callId": call_id,
                                });
                                let _ = app_handle.emit("sip-conference-split", payload);
                            }
                            sip::SipEvent::ConferenceEnded { conference_id } => {
                                let payload = serde_json::json!({
                                    "conferenceId": conference_id,
                                });
                                let _ = app_handle.emit("sip-conference-ended", payload);
                            }
                            sip::SipEvent::RecordingStateChanged(rec) => {
                                let _ = app_handle.emit("sip-recording", rec);
                            }
                            sip::SipEvent::VoicemailStatus(vm) => {
                                let payload = serde_json::json!({
                                    "accountId": vm.account_id,
                                    "hasMessages": vm.messages_waiting,
                                    "newMessages": vm.new_messages,
                                    "oldMessages": vm.old_messages,
                                });
                                let _ = app_handle.emit("sip-voicemail", payload);
                            }
                        }
                    }
                log::warn!("SIP event forwarding loop ended");
            });
            
            // Resolve the recordings directory once, here, where the app data
            // dir is available. The auto-record paths used to build their own
            // from a hardcoded identifier that did not match the bundle's, so
            // auto-recordings were written somewhere the UI never looked.
            match app.path().app_data_dir() {
                Ok(data_dir) => {
                    let recordings_manager = app.state::<sip::SipManager>().inner().clone();
                    let recordings_dir = data_dir.join("recordings");
                    // Block rather than spawn: this must be in place before any
                    // call can be placed or answered, or auto-record would skip
                    // the first call of the session.
                    tauri::async_runtime::block_on(
                        recordings_manager.set_recordings_dir(recordings_dir),
                    );
                }
                Err(e) => log::error!(
                    "Could not resolve app data dir; call recording is disabled: {}",
                    e
                ),
            }

            // The local MCP server for assistants. Off unless the user turned
            // it on; the state is managed either way so Settings can turn it
            // on later. Spawned, not blocked on: binding the port and reading
            // the token from the keychain must not hold up the window.
            // The Diagnostics window docks to the main window and follows
            // it; see diagnostics_window.rs.
            app.manage(diagnostics_window::DiagnosticsWindow::load(app.path().app_data_dir().ok()));
            if let Some(main) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                main.on_window_event(move |event| match event {
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                        diagnostics_window::reposition(&handle);
                    }
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed => {
                        diagnostics_window::close(&handle);
                    }
                    _ => {}
                });
            }

            match app.path().app_data_dir() {
                Ok(data_dir) => {
                    let sip_manager = app.state::<sip::SipManager>().inner().clone();
                    app.manage(mcp::McpState::new(app.handle().clone(), sip_manager, data_dir));
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        handle.state::<mcp::McpState>().start_if_enabled().await;
                    });
                }
                Err(e) => log::error!("Could not resolve app data dir; MCP server is unavailable: {}", e),
            }

            // Listen for health probe requests from frontend (network/visibility changes)
            let probe_manager = app.state::<sip::SipManager>().inner().clone();
            app.listen("probe-health", move |_event| {
                let manager = probe_manager.clone();
                tauri::async_runtime::spawn(async move {
                    manager.probe_registration_health().await;
                });
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::provision_from_qr,
            commands::take_launch_links,
            provision::provision_link_preview,
            provision::provision_link_redeem,
            commands::sip_register,
            commands::sip_register_now,
            commands::sip_unregister,
            commands::sip_unregister_account,
            commands::sip_set_active_account,
            commands::sip_make_call,
            commands::sip_hangup,
            commands::sip_answer,
            commands::sip_hold,
            commands::sip_mute,
            commands::sip_send_dtmf,
            commands::sip_start_recording,
            commands::sip_stop_recording,
            commands::sip_is_recording,
            commands::get_default_recordings_dir,
            commands::open_recordings_folder,
            commands::play_recording,
            commands::get_registration_state,
            commands::get_connection_report,
            commands::probe_registration_health,
            commands::process_pending_resubscriptions,
            commands::get_audio_devices,
            commands::set_audio_devices,
            commands::get_voice_processing,
            commands::set_voice_processing,
            commands::open_debug_window,
            commands::diagnostics_docked,
            commands::diagnostics_set_docked,
            commands::get_app_info,
            commands::sip_reregister,
            commands::app_build_info,
            commands::get_sip_diagnostics,
            commands::clear_sip_diagnostics,
            commands::get_rtp_stats,
            commands::get_audio_levels,
            commands::sip_add_call,
            commands::sip_conference_merge,
            commands::sip_conference_split,
            commands::sip_conference_end,
            commands::sip_swap_calls,
            commands::sip_transfer_blind,
            commands::sip_transfer_attended,
            commands::sip_subscribe_blf,
            commands::sip_unsubscribe_blf,
            commands::get_blf_states,
            commands::get_system_status,
            commands::export_sip_log_text,
            commands::export_sip_log_pcap,
            commands::export_call_pcap,
            commands::get_call_sip_trace,
            commands::store_credential,
            commands::get_credential,
            commands::delete_credential,
            commands::fetch_system_contacts,
            commands::open_contacts_privacy_settings,
            commands::export_diagnostic_report,
            commands::export_call_history_csv,
            commands::start_audio_test,
            commands::stop_audio_test,
            commands::play_test_tone,
            commands::ai_available,
            commands::ai_models,
            commands::ai_start_download,
            commands::ai_download_progress,
            commands::ai_cancel_download,
            commands::ai_delete_model,
            commands::ai_transcribe_recording,
            commands::ai_insights,
            commands::ai_insight,
            commands::ai_delete_insight,
            commands::ai_clear_insights,
            commands::ai_auto_transcribe,
            commands::ai_set_auto_transcribe,
            commands::voicemail_list,
            commands::voicemail_mark_read,
            commands::voicemail_delete,
            commands::voicemail_audio,
            mcp::mcp_get_config,
            mcp::mcp_set_config,
            mcp::mcp_get_token,
            mcp::mcp_rotate_token,
            mcp::mcp_call_decision,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Aria")
        .run(|app_handle, event| {
            match event {
                // Clicking the Dock icon (or launching Aria again) while its
                // window is hidden or minimized must bring the window back and
                // focus it.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
                // Quitting (last window closed, Cmd+Q) is `code: None`; the
                // `app.exit(0)` below comes back as `Some(0)` and goes
                // through. Until the bindings are cleared the PBX keeps
                // sending calls to a phone that is no longer there, for as
                // long as the registration had left to run.
                tauri::RunEvent::ExitRequested { api, code: None, .. } => {
                    api.prevent_exit();
                    let handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let manager = handle.state::<sip::SipManager>().inner().clone();
                        // Bounded: a registrar that does not answer must not
                        // keep the app open.
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(4),
                            manager.unregister_all(),
                        )
                        .await;
                        handle.exit(0);
                    });
                }
                _ => {}
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BUNDLE_IDENTIFIER` decides where data written outside Tauri's own path
    /// APIs lands. If it stops matching tauri.conf.json, those files silently
    /// move to a directory nothing else reads — which is exactly how
    /// auto-recordings ended up invisible.
    #[test]
    fn bundle_identifier_matches_config() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json"))
                .expect("tauri.conf.json should be valid JSON");
        let identifier = config["identifier"]
            .as_str()
            .expect("tauri.conf.json should have a string identifier");

        assert_eq!(
            identifier, BUNDLE_IDENTIFIER,
            "BUNDLE_IDENTIFIER must match tauri.conf.json's identifier"
        );
    }

    #[test]
    fn app_data_root_ends_with_bundle_identifier() {
        let root = app_data_root().expect("test platforms have a data dir");
        assert_eq!(
            root.file_name().and_then(|n| n.to_str()),
            Some(BUNDLE_IDENTIFIER)
        );
    }

    /// A missing probe entry is a working store; only a store that cannot be
    /// reached sends Linux to the session keyring.
    #[test]
    fn only_an_unreachable_store_counts_as_unavailable() {
        let failure = || -> Box<dyn std::error::Error + Send + Sync> { "no secret service".into() };
        assert!(!credential_store_unavailable(&Ok("x".into())));
        assert!(!credential_store_unavailable(&Err(keyring::Error::NoEntry)));
        assert!(credential_store_unavailable(&Err(keyring::Error::PlatformFailure(failure()))));
        assert!(credential_store_unavailable(&Err(keyring::Error::NoStorageAccess(failure()))));
    }
}
