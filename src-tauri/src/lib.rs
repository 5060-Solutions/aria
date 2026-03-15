mod commands;
mod sip;
mod system_contacts;

use tauri::Emitter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Create manager and take receiver before moving into Tauri
    let (manager, event_rx) = sip::SipManager::new_with_receiver();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {
            // Handle single instance - argv may contain deep link URL
        }))
        .manage(manager)
        .setup(move |app| {
            // Set up event forwarding from SIP manager to frontend ONCE at startup
            let app_handle = app.handle().clone();
            
            // Spawn the event forwarding task with the receiver we took earlier
            let mut rx = event_rx;
            tauri::async_runtime::spawn(async move {
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
                            sip::SipEvent::CallStateChanged(call_event) => {
                                let _ = app_handle.emit("sip-call", call_event);
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
                        }
                    }
                log::warn!("SIP event forwarding loop ended");
            });
            
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::sip_register,
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
            commands::get_audio_devices,
            commands::open_debug_window,
            commands::get_sip_diagnostics,
            commands::clear_sip_diagnostics,
            commands::get_rtp_stats,
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
            commands::export_diagnostic_report,
            commands::export_call_history_csv,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aria");
}
