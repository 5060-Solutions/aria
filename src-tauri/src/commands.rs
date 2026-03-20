use serde::Deserialize;
use tauri::{Manager, State};

use crate::audio_test::AudioTestManager;
use crate::sip::{self, SipManager};

// ── PCAP helpers ────────────────────────────────────────────────────────────

fn write_u8(buf: &mut Vec<u8>, v: u8) { buf.push(v); }
fn write_u16le(buf: &mut Vec<u8>, v: u16) { buf.extend_from_slice(&v.to_le_bytes()); }
fn write_u32le(buf: &mut Vec<u8>, v: u32) { buf.extend_from_slice(&v.to_le_bytes()); }
fn write_i32le(buf: &mut Vec<u8>, v: i32) { buf.extend_from_slice(&v.to_le_bytes()); }

/// Build a minimal PCAP file (LINKTYPE_IPV4 = 228) from SIP diagnostic logs.
/// Each SIP message is wrapped in a fake IPv4 + UDP header so Wireshark
/// dissects it correctly.
fn build_pcap(logs: &[sip::diagnostics::DiagnosticLog]) -> Vec<u8> {
    let mut buf = Vec::new();

    // Global header
    write_u32le(&mut buf, 0xa1b2c3d4); // magic
    write_u16le(&mut buf, 2);           // version major
    write_u16le(&mut buf, 4);           // version minor
    write_i32le(&mut buf, 0);           // UTC offset
    write_u32le(&mut buf, 0);           // timestamp accuracy
    write_u32le(&mut buf, 65535);       // snaplen
    write_u32le(&mut buf, 228);         // LINKTYPE_IPV4

    for log in logs {
        let payload = log.raw.as_bytes();
        let sip_len = payload.len();
        let udp_len = 8 + sip_len;
        let ip_len = 20 + udp_len;

        // Parse remote addr; fall back to a placeholder if invalid
        let remote: std::net::SocketAddr = log
            .remote_addr
            .parse()
            .unwrap_or_else(|_| "1.2.3.4:5060".parse().unwrap());

        let remote_ip = match remote.ip() {
            std::net::IpAddr::V4(a) => a.octets(),
            std::net::IpAddr::V6(_) => [127, 0, 0, 1],
        };
        let localhost = [127u8, 0, 0, 1];
        let is_sent = matches!(log.direction, sip::diagnostics::MessageDirection::Sent);
        let (src_v4, src_port, dst_v4, dst_port) = if is_sent {
            (localhost, 5060u16, remote_ip, remote.port())
        } else {
            (remote_ip, remote.port(), localhost, 5060u16)
        };

        // Packet record header
        let ts_sec = (log.timestamp / 1000) as u32;
        let ts_usec = ((log.timestamp % 1000) * 1000) as u32;
        write_u32le(&mut buf, ts_sec);
        write_u32le(&mut buf, ts_usec);
        write_u32le(&mut buf, ip_len as u32);
        write_u32le(&mut buf, ip_len as u32);

        // IPv4 header (20 bytes, no options)
        write_u8(&mut buf, 0x45);                        // version=4, IHL=5
        write_u8(&mut buf, 0x00);                        // DSCP/ECN
        write_u16le(&mut buf, ip_len as u16);            // total length
        write_u16le(&mut buf, 0);                        // identification
        write_u16le(&mut buf, 0x4000);                   // flags=DF, frag=0
        write_u8(&mut buf, 64);                          // TTL
        write_u8(&mut buf, 17);                          // protocol=UDP
        write_u16le(&mut buf, 0);                        // checksum (0 = unchecked)
        buf.extend_from_slice(&src_v4);
        buf.extend_from_slice(&dst_v4);

        // UDP header (8 bytes)
        write_u16le(&mut buf, src_port);
        write_u16le(&mut buf, dst_port);
        write_u16le(&mut buf, udp_len as u16);
        write_u16le(&mut buf, 0);                        // checksum (0 = unchecked)

        // SIP payload
        buf.extend_from_slice(payload);
    }

    buf
}

/// QR provisioning payload from the frontend
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QrProvisionPayload {
    pub server: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
    pub transport: Option<String>,
    pub voicemail: Option<String>,
}

/// Provision an account from a scanned QR code or pasted aria:// URI.
/// The frontend parses the URI and sends the structured payload.
#[tauri::command]
pub async fn provision_from_qr(
    payload: QrProvisionPayload,
    manager: State<'_, SipManager>,
) -> Result<String, String> {
    log::info!(
        "provision_from_qr: server={}, username={}, port={}, transport={:?}",
        payload.server, payload.username, payload.port, payload.transport
    );

    let transport = match payload.transport.as_deref().unwrap_or("udp") {
        "udp" => sip::TransportType::Udp,
        "tcp" => sip::TransportType::Tcp,
        "tls" => sip::TransportType::Tls,
        _ => sip::TransportType::Udp,
    };

    let account_id = uuid::Uuid::new_v4().to_string();
    let display_name = payload.display_name.unwrap_or_else(|| payload.username.clone());

    let account = sip::AccountConfig {
        id: account_id.clone(),
        display_name,
        username: payload.username.clone(),
        domain: payload.server.clone(),
        password: payload.password,
        transport,
        port: payload.port,
        registrar: Some(payload.server),
        outbound_proxy: None,
        auth_username: None,
        auth_realm: None,
        enabled: true,
        auto_record: true,
        srtp_mode: Default::default(),
        codecs: sip::account::default_codec_preferences(),
    };

    manager.register(account).await?;
    Ok(account_id)
}

/// Codec configuration from frontend
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecConfigEntry {
    pub codec: String,
    pub enabled: bool,
    pub priority: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SipAccountConfig {
    pub id: Option<String>,
    pub display_name: String,
    pub username: String,
    pub domain: String,
    pub password: String,
    pub transport: String,
    pub port: u16,
    pub registrar: Option<String>,
    pub outbound_proxy: Option<String>,
    pub auth_username: Option<String>,
    pub auth_realm: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_auto_record")]
    pub auto_record: bool,
    #[serde(default)]
    pub srtp_mode: Option<String>,
    #[serde(default)]
    pub codecs: Option<Vec<CodecConfigEntry>>,
}

fn default_enabled() -> bool {
    true
}

fn default_auto_record() -> bool {
    true // Auto-record enabled by default as per user request
}

#[tauri::command]
pub async fn sip_register(
    config: SipAccountConfig,
    manager: State<'_, SipManager>,
) -> Result<String, String> {
    log::info!(
        "sip_register called: username={}, domain={}, transport={}, port={}, has_password={}, password_len={}",
        config.username,
        config.domain,
        config.transport,
        config.port,
        !config.password.is_empty(),
        config.password.len()
    );

    let transport = match config.transport.as_str() {
        "udp" => sip::TransportType::Udp,
        "tcp" => sip::TransportType::Tcp,
        "tls" => sip::TransportType::Tls,
        _ => return Err("Invalid transport type".into()),
    };

    // Parse codec preferences from config
    let codecs = config.codecs.map(|codec_list| {
        let mut prefs: Vec<sip::account::CodecPreference> = codec_list
            .into_iter()
            .filter_map(|c| {
                let codec = match c.codec.as_str() {
                    "pcmu" => Some(rtp_engine::CodecType::Pcmu),
                    "pcma" => Some(rtp_engine::CodecType::Pcma),
                    "g729" => Some(rtp_engine::CodecType::G729),
                    "opus" => Some(rtp_engine::CodecType::Opus),
                    _ => None,
                };
                codec.map(|ct| sip::account::CodecPreference {
                    codec: ct,
                    enabled: c.enabled,
                    priority: c.priority,
                })
            })
            .collect();
        prefs.sort_by_key(|p| p.priority);
        prefs
    }).unwrap_or_else(sip::account::default_codec_preferences);

    let srtp_mode = config.srtp_mode
        .as_ref()
        .map(|s| sip::account::SrtpMode::from_str(s))
        .unwrap_or_default();
    log::info!("Registering account {} with SRTP mode: {:?} (from config: {:?})", 
               config.username, srtp_mode, config.srtp_mode);

    let account = sip::AccountConfig {
        id: config.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        display_name: config.display_name,
        username: config.username,
        domain: config.domain,
        password: config.password,
        transport,
        port: config.port,
        registrar: config.registrar,
        outbound_proxy: config.outbound_proxy,
        auth_username: config.auth_username,
        auth_realm: config.auth_realm,
        enabled: config.enabled,
        auto_record: config.auto_record,
        srtp_mode,
        codecs,
    };

    manager.register(account).await
}

#[tauri::command]
pub async fn sip_unregister(manager: State<'_, SipManager>) -> Result<(), String> {
    manager.unregister().await
}

#[tauri::command]
pub async fn sip_unregister_account(
    account_id: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.unregister_account(Some(&account_id)).await
}

#[tauri::command]
pub async fn sip_set_active_account(
    account_id: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.set_active_account(&account_id).await
}

#[tauri::command]
pub async fn sip_make_call(uri: String, manager: State<'_, SipManager>) -> Result<String, String> {
    manager.make_call(&uri).await
}

#[tauri::command]
pub async fn sip_hangup(call_id: String, manager: State<'_, SipManager>) -> Result<(), String> {
    manager.hangup(&call_id).await
}

#[tauri::command]
pub async fn sip_answer(call_id: String, manager: State<'_, SipManager>) -> Result<(), String> {
    manager.answer(&call_id).await
}

#[tauri::command]
pub async fn sip_hold(
    call_id: String,
    hold: bool,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.hold(&call_id, hold).await
}

#[tauri::command]
pub async fn sip_mute(
    call_id: String,
    mute: bool,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.mute(&call_id, mute).await
}

#[tauri::command]
pub async fn sip_send_dtmf(
    call_id: String,
    digit: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.send_dtmf(&call_id, &digit).await
}

#[tauri::command]
pub async fn sip_start_recording(
    call_id: String,
    manager: State<'_, SipManager>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    // Get the recordings directory (inside app data dir)
    let recordings_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?
        .join("recordings");
    
    manager.start_recording(&call_id, &recordings_dir).await
}

#[tauri::command]
pub async fn sip_stop_recording(
    call_id: String,
    manager: State<'_, SipManager>,
) -> Result<Option<String>, String> {
    manager.stop_recording(&call_id).await
}

#[tauri::command]
pub async fn sip_is_recording(
    call_id: String,
    manager: State<'_, SipManager>,
) -> Result<bool, String> {
    manager.is_recording(&call_id).await
}

#[tauri::command]
pub async fn get_default_recordings_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?
        .join("recordings");
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_recordings_folder(
    custom_path: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let dir = if let Some(path) = custom_path {
        std::path::PathBuf::from(path)
    } else {
        app.path()
            .app_data_dir()
            .map_err(|e| format!("Failed to get app data dir: {}", e))?
            .join("recordings")
    };
    
    // Create directory if it doesn't exist
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create dir: {}", e))?;
    
    // Open in file explorer
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }
    
    Ok(())
}

#[tauri::command]
pub async fn play_recording(path: String) -> Result<(), String> {
    let path = std::path::Path::new(&path);
    if !path.exists() {
        return Err("Recording file not found".into());
    }
    
    // Open with default audio player
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to play recording: {}", e))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn()
            .map_err(|e| format!("Failed to play recording: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("Failed to play recording: {}", e))?;
    }
    
    Ok(())
}

#[tauri::command]
pub async fn get_registration_state(
    manager: State<'_, SipManager>,
) -> Result<serde_json::Value, String> {
    let (state, error) = manager.registration_state().await;
    Ok(serde_json::json!({
        "state": state,
        "error": error,
    }))
}

/// Probe registration health for all accounts.
/// Sends an immediate OPTIONS ping; triggers reconnection on failure.
/// Called by frontend on wake from sleep, network online, or window re-focus.
#[tauri::command]
pub async fn probe_registration_health(
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.probe_registration_health().await;
    Ok(())
}

/// Audio device information returned to frontend
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// Available audio devices
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevicesResponse {
    pub input_devices: Vec<AudioDeviceInfo>,
    pub output_devices: Vec<AudioDeviceInfo>,
}

#[tauri::command]
pub async fn set_audio_devices(
    manager: State<'_, SipManager>,
    input_device: Option<String>,
    output_device: Option<String>,
) -> Result<(), String> {
    manager.set_audio_devices(input_device, output_device).await;
    Ok(())
}

#[tauri::command]
pub async fn get_audio_devices() -> Result<AudioDevicesResponse, String> {
    let devices = rtp_engine::list_all_devices()
        .map_err(|e| format!("Failed to list audio devices: {}", e))?;

    Ok(AudioDevicesResponse {
        input_devices: devices.input_devices.into_iter().map(|d| AudioDeviceInfo {
            name: d.name,
            is_default: d.is_default,
        }).collect(),
        output_devices: devices.output_devices.into_iter().map(|d| AudioDeviceInfo {
            name: d.name,
            is_default: d.is_default,
        }).collect(),
    })
}

#[tauri::command]
pub async fn open_debug_window(app: tauri::AppHandle) -> Result<(), String> {
    // Check if window already exists
    if let Some(window) = app.get_webview_window("debug") {
        window
            .set_focus()
            .map_err(|e| format!("Focus error: {}", e))?;
        return Ok(());
    }

    // Position flush to the right of the main window, same vertical origin.
    // outer_position/outer_size return physical pixels; divide by scale factor
    // to get logical coordinates that the window builder expects.
    let (x, y) = app
        .get_webview_window("main")
        .and_then(|w| {
            let scale = w.scale_factor().ok()?;
            let pos = w.outer_position().ok()?;
            let size = w.outer_size().ok()?;
            let x = (pos.x as f64 + size.width as f64) / scale + 8.0;
            let y = pos.y as f64 / scale;
            Some((x, y))
        })
        .unwrap_or((100.0, 100.0));

    tauri::WebviewWindowBuilder::new(&app, "debug", tauri::WebviewUrl::App("/".into()))
        .title("Aria — Developer")
        .inner_size(520.0, 700.0)
        .min_inner_size(400.0, 500.0)
        .position(x, y)
        .build()
        .map_err(|e| format!("Failed to open debug window: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn get_sip_diagnostics(
    manager: State<'_, SipManager>,
) -> Result<Vec<serde_json::Value>, String> {
    let logs = manager.get_diagnostics().await;
    Ok(logs
        .into_iter()
        .map(|l| serde_json::to_value(l).unwrap_or_default())
        .collect())
}

#[tauri::command]
pub async fn clear_sip_diagnostics(manager: State<'_, SipManager>) -> Result<(), String> {
    manager.clear_diagnostics().await;
    Ok(())
}

#[tauri::command]
pub async fn get_rtp_stats(manager: State<'_, SipManager>) -> Result<serde_json::Value, String> {
    match manager.get_rtp_stats().await {
        Some(stats) => Ok(serde_json::to_value(stats).unwrap_or_default()),
        None => Ok(serde_json::json!(null)),
    }
}

/// Get live audio levels for the active call (mic TX and speaker RX, RMS 0.0-1.0).
/// Also returns levels from the audio test manager when a test is running.
#[tauri::command]
pub async fn get_audio_levels(
    manager: State<'_, SipManager>,
    test_manager: State<'_, AudioTestManager>,
) -> Result<serde_json::Value, String> {
    // First try active call levels
    if let Some((tx, rx)) = manager.get_audio_levels().await {
        return Ok(serde_json::json!({ "tx": tx, "rx": rx }));
    }

    // Fall back to test levels if a mic test is running
    if let Some(tx) = test_manager.get_test_level() {
        return Ok(serde_json::json!({ "tx": tx, "rx": 0.0 }));
    }

    Ok(serde_json::json!(null))
}

/// Make a second call while the first call is on hold (for three-way calling)
#[tauri::command]
pub async fn sip_add_call(
    uri: String,
    manager: State<'_, SipManager>,
) -> Result<String, String> {
    // This creates a second call - the frontend should have already put the first call on hold
    manager.make_call(&uri).await
}

/// Merge two or more calls into a local conference
#[tauri::command]
pub async fn sip_conference_merge(
    call_ids: Vec<String>,
    manager: State<'_, SipManager>,
) -> Result<String, String> {
    manager.conference_merge(&call_ids).await
}

/// Split a call from a conference
#[tauri::command]
pub async fn sip_conference_split(
    conference_id: String,
    call_id: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.conference_split(&conference_id, &call_id).await
}

/// End a conference (hangs up all calls in the conference)
#[tauri::command]
pub async fn sip_conference_end(
    conference_id: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.conference_end(&conference_id).await
}

/// Swap between two calls (put current on hold, resume other)
#[tauri::command]
pub async fn sip_swap_calls(
    hold_call_id: String,
    resume_call_id: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.swap_calls(&hold_call_id, &resume_call_id).await
}

#[tauri::command]
pub async fn sip_transfer_blind(
    call_id: String,
    target_uri: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.transfer_blind(&call_id, &target_uri).await
}

#[tauri::command]
pub async fn sip_transfer_attended(
    call_id_a: String,
    call_id_b: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    manager.transfer_attended(&call_id_a, &call_id_b).await
}

#[tauri::command]
pub async fn sip_subscribe_blf(
    extensions: Vec<String>,
    manager: State<'_, SipManager>,
) -> Result<Vec<String>, String> {
    let domain = {
        let (state, _) = manager.registration_state().await;
        if state != sip::RegistrationState::Registered {
            return Err("Not registered".into());
        }
        manager.get_domain().await.ok_or("No account configured")?
    };

    let mut sub_ids = Vec::new();
    for ext in &extensions {
        let target_uri = format!("sip:{}@{}", ext, domain);
        match manager
            .subscribe_presence(&target_uri, sip::presence::EventType::Dialog)
            .await
        {
            Ok(id) => sub_ids.push(id),
            Err(e) => {
                log::error!("Failed to subscribe BLF for {}: {}", ext, e);
            }
        }
    }

    Ok(sub_ids)
}

#[tauri::command]
pub async fn sip_unsubscribe_blf(
    extension: String,
    manager: State<'_, SipManager>,
) -> Result<(), String> {
    let sub_id = manager.find_subscription_by_extension(&extension).await;
    match sub_id {
        Some(id) => manager.unsubscribe(&id).await,
        None => Err(format!("No subscription found for extension {}", extension)),
    }
}

#[tauri::command]
pub async fn export_sip_log_text(
    manager: State<'_, SipManager>,
    path: Option<String>,
) -> Result<String, String> {
    let logs = manager.get_diagnostics().await;
    let mut out = String::new();
    for log in &logs {
        let dir = match log.direction {
            sip::diagnostics::MessageDirection::Sent => ">>> SENT",
            sip::diagnostics::MessageDirection::Received => "<<< RECEIVED",
        };
        let ts = chrono_format(log.timestamp);
        out.push_str(&format!("── {} {} {} ──\n{}\n\n", dir, ts, log.remote_addr, log.raw));
    }
    
    if let Some(file_path) = path {
        std::fs::write(&file_path, &out)
            .map_err(|e| format!("Failed to write file: {}", e))?;
        Ok(file_path)
    } else {
        Ok(out)
    }
}

#[tauri::command]
pub async fn export_sip_log_pcap(
    manager: State<'_, SipManager>,
    path: Option<String>,
) -> Result<String, String> {
    let logs = manager.get_diagnostics().await;
    let bytes = build_pcap(&logs);
    
    if let Some(file_path) = path {
        std::fs::write(&file_path, &bytes)
            .map_err(|e| format!("Failed to write file: {}", e))?;
        Ok(file_path)
    } else {
        Ok(base64_encode(&bytes))
    }
}

fn chrono_format(ts_millis: u64) -> String {
    let secs = ts_millis / 1000;
    let ms = ts_millis % 1000;
    // Simple UTC formatting without external crate
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[(n >> 18 & 0x3F) as usize] as char);
        out.push(CHARS[(n >> 12 & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { CHARS[(n >> 6 & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { CHARS[(n & 0x3F) as usize] as char } else { '=' });
    }
    out
}

#[tauri::command]
pub async fn get_system_status(
    manager: State<'_, SipManager>,
) -> Result<sip::SystemStatus, String> {
    Ok(manager.get_system_status().await)
}

#[tauri::command]
pub async fn get_blf_states(
    manager: State<'_, SipManager>,
) -> Result<Vec<sip::presence::BlfEntry>, String> {
    Ok(manager.get_blf_states().await)
}

// ── Secure Credential Storage ───────────────────────────────────────────────
//
// When `dev-insecure` feature is enabled, uses a plain JSON file instead of
// the OS keychain. This avoids constant keychain prompts during development.
// WARNING: Never use dev-insecure in production!

#[cfg(not(feature = "dev-insecure"))]
const KEYRING_SERVICE: &str = "aria-softphone";

/// Get the path to the insecure credentials file (dev only)
#[cfg(feature = "dev-insecure")]
fn insecure_creds_path() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("com.5060.aria")
        .join(".dev-credentials.json")
}

/// Load credentials from insecure file storage (dev only)
#[cfg(feature = "dev-insecure")]
fn load_insecure_creds() -> std::collections::HashMap<String, String> {
    let path = insecure_creds_path();
    if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    }
}

/// Save credentials to insecure file storage (dev only)
#[cfg(feature = "dev-insecure")]
fn save_insecure_creds(creds: &std::collections::HashMap<String, String>) -> Result<(), String> {
    let path = insecure_creds_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create credentials directory: {}", e))?;
    }
    let json = serde_json::to_string_pretty(creds)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write credentials file: {}", e))?;
    Ok(())
}

/// Store a password securely in the OS keychain (or insecure file in dev mode)
#[tauri::command]
pub fn store_credential(account_id: &str, password: &str) -> Result<(), String> {
    log::info!("store_credential: account_id={}, password_len={}", account_id, password.len());
    
    #[cfg(feature = "dev-insecure")]
    {
        log::warn!("⚠️  Using INSECURE file-based credential storage (dev mode)");
        let mut creds = load_insecure_creds();
        creds.insert(account_id.to_string(), password.to_string());
        save_insecure_creds(&creds)?;
        log::info!("store_credential: successfully stored for account_id={}", account_id);
        return Ok(());
    }
    
    #[cfg(not(feature = "dev-insecure"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account_id)
            .map_err(|e| format!("Failed to create keyring entry: {}", e))?;
        entry
            .set_password(password)
            .map_err(|e| format!("Failed to store credential: {}", e))?;
        log::info!("store_credential: successfully stored for account_id={}", account_id);
        Ok(())
    }
}

/// Retrieve a password from the OS keychain (or insecure file in dev mode)
#[tauri::command]
pub fn get_credential(account_id: &str) -> Result<Option<String>, String> {
    log::info!("get_credential: account_id={}", account_id);
    
    #[cfg(feature = "dev-insecure")]
    {
        let creds = load_insecure_creds();
        let result = creds.get(account_id).cloned();
        if result.is_some() {
            log::info!("get_credential: found password for account_id={}", account_id);
        } else {
            log::warn!("get_credential: no entry found for account_id={}", account_id);
        }
        return Ok(result);
    }
    
    #[cfg(not(feature = "dev-insecure"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account_id)
            .map_err(|e| format!("Failed to create keyring entry: {}", e))?;
        match entry.get_password() {
            Ok(password) => {
                log::info!("get_credential: found password for account_id={}, len={}", account_id, password.len());
                Ok(Some(password))
            }
            Err(keyring::Error::NoEntry) => {
                log::warn!("get_credential: no entry found for account_id={}", account_id);
                Ok(None)
            }
            Err(e) => {
                log::error!("get_credential: error for account_id={}: {}", account_id, e);
                Err(format!("Failed to retrieve credential: {}", e))
            }
        }
    }
}

/// Delete a password from the OS keychain (or insecure file in dev mode)
#[tauri::command]
pub fn delete_credential(account_id: &str) -> Result<(), String> {
    #[cfg(feature = "dev-insecure")]
    {
        let mut creds = load_insecure_creds();
        creds.remove(account_id);
        return save_insecure_creds(&creds);
    }
    
    #[cfg(not(feature = "dev-insecure"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account_id)
            .map_err(|e| format!("Failed to create keyring entry: {}", e))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already deleted, that's fine
            Err(e) => Err(format!("Failed to delete credential: {}", e)),
        }
    }
}

// ── System Contacts ─────────────────────────────────────────────────────────

/// Fetch contacts from the system contacts database (macOS only)
#[tauri::command]
pub fn fetch_system_contacts() -> Result<Vec<crate::system_contacts::SystemContact>, String> {
    crate::system_contacts::fetch_contacts()
}

// ── Comprehensive Diagnostic Export ─────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHistoryEntry {
    pub id: String,
    pub remote_uri: String,
    pub remote_name: Option<String>,
    pub direction: String,
    pub start_time: u64,
    pub duration: u64,
    pub missed: bool,
    pub recording_path: Option<String>,
}

/// Export a comprehensive diagnostic report as JSON
#[tauri::command]
pub async fn export_diagnostic_report(
    manager: State<'_, SipManager>,
    call_history: Vec<CallHistoryEntry>,
    path: String,
) -> Result<String, String> {
    let system_status = manager.get_system_status().await;
    let sip_logs = manager.get_diagnostics().await;
    
    let report = serde_json::json!({
        "exportedAt": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        "version": "0.2.0",
        "systemStatus": system_status,
        "sipMessageCount": sip_logs.len(),
        "callHistory": call_history,
        "callHistoryCount": call_history.len(),
    });
    
    let json_str = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("Failed to serialize report: {}", e))?;
    
    std::fs::write(&path, &json_str)
        .map_err(|e| format!("Failed to write file: {}", e))?;
    
    Ok(path)
}

/// Export PCAP for a specific call (filtered by SIP Call-ID)
#[tauri::command]
pub async fn export_call_pcap(
    manager: State<'_, SipManager>,
    sip_call_id: String,
    path: Option<String>,
) -> Result<String, String> {
    let logs = manager.get_diagnostics().await;
    let filtered: Vec<_> = logs
        .into_iter()
        .filter(|l| l.call_id.as_deref() == Some(&sip_call_id))
        .collect();

    if filtered.is_empty() {
        return Err("No SIP messages found for this call".into());
    }

    let bytes = build_pcap(&filtered);

    if let Some(file_path) = path {
        std::fs::write(&file_path, &bytes)
            .map_err(|e| format!("Failed to write file: {}", e))?;
        Ok(file_path)
    } else {
        Ok(base64_encode(&bytes))
    }
}

/// Get SIP message trace for a specific call
#[tauri::command]
pub async fn get_call_sip_trace(
    manager: State<'_, SipManager>,
    sip_call_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let logs = manager.get_diagnostics().await;
    let filtered: Vec<_> = logs
        .into_iter()
        .filter(|l| l.call_id.as_deref() == Some(&sip_call_id))
        .map(|l| serde_json::to_value(l).unwrap_or_default())
        .collect();
    Ok(filtered)
}

/// Export call history as CSV
#[tauri::command]
pub async fn export_call_history_csv(
    call_history: Vec<CallHistoryEntry>,
    path: String,
) -> Result<String, String> {
    let mut csv = String::from("ID,Remote URI,Remote Name,Direction,Start Time,Duration (s),Missed,Recording Path\n");
    
    for entry in &call_history {
        csv.push_str(&format!(
            "\"{}\",\"{}\",\"{}\",\"{}\",{},{},{},\"{}\"\n",
            entry.id,
            entry.remote_uri,
            entry.remote_name.as_deref().unwrap_or(""),
            entry.direction,
            entry.start_time,
            entry.duration,
            entry.missed,
            entry.recording_path.as_deref().unwrap_or("")
        ));
    }
    
    std::fs::write(&path, &csv)
        .map_err(|e| format!("Failed to write file: {}", e))?;

    Ok(path)
}

// ── Audio Device Testing ─────────────────────────────────────────────────────

/// Start capturing from the selected input device and measuring RMS levels.
/// The levels are returned by `get_audio_levels` when no call is active.
#[tauri::command]
pub fn start_audio_test(
    test_manager: State<'_, AudioTestManager>,
    device_name: Option<String>,
) -> Result<(), String> {
    test_manager.start_input_test(device_name.as_deref())
}

/// Stop the input device test.
#[tauri::command]
pub fn stop_audio_test(test_manager: State<'_, AudioTestManager>) -> Result<(), String> {
    test_manager.stop_input_test();
    Ok(())
}

/// Play a brief test tone through the selected output device.
#[tauri::command]
pub fn play_test_tone(
    test_manager: State<'_, AudioTestManager>,
    device_name: Option<String>,
) -> Result<(), String> {
    test_manager.play_test_tone(device_name.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sip::diagnostics::{DiagnosticLog, MessageDirection};

    fn make_log(call_id: Option<&str>, direction: MessageDirection, remote: &str) -> DiagnosticLog {
        DiagnosticLog {
            timestamp: 1710000000000,
            account_id: "test-account".to_string(),
            direction,
            remote_addr: remote.to_string(),
            summary: "INVITE sip:bob@example.com SIP/2.0".to_string(),
            raw: format!(
                "INVITE sip:bob@example.com SIP/2.0\r\n\
                 Call-ID: {}\r\n\
                 \r\n",
                call_id.unwrap_or("no-id")
            ),
            call_id: call_id.map(String::from),
        }
    }

    #[test]
    fn test_build_pcap_produces_valid_header() {
        let logs = vec![make_log(Some("call-1"), MessageDirection::Sent, "10.0.0.1:5060")];
        let pcap = build_pcap(&logs);

        // PCAP magic number (little-endian): 0xa1b2c3d4
        assert_eq!(pcap[0..4], [0xd4, 0xc3, 0xb2, 0xa1]);
        // Version 2.4
        assert_eq!(pcap[4..6], 2u16.to_le_bytes());
        assert_eq!(pcap[6..8], 4u16.to_le_bytes());
        // Link type 228 (LINKTYPE_IPV4)
        assert_eq!(pcap[20..24], 228u32.to_le_bytes());
    }

    #[test]
    fn test_build_pcap_empty_logs() {
        let pcap = build_pcap(&[]);
        // Just global header (24 bytes)
        assert_eq!(pcap.len(), 24);
    }

    #[test]
    fn test_build_pcap_sent_vs_received_direction() {
        let sent = vec![make_log(Some("c1"), MessageDirection::Sent, "10.0.0.2:5060")];
        let recv = vec![make_log(Some("c1"), MessageDirection::Received, "10.0.0.2:5060")];

        let pcap_sent = build_pcap(&sent);
        let pcap_recv = build_pcap(&recv);

        // Both should produce valid PCAPs with different IP address ordering
        assert!(pcap_sent.len() > 24);
        assert!(pcap_recv.len() > 24);
        // The IP headers should differ (source/dest swapped)
        assert_ne!(pcap_sent[24..], pcap_recv[24..]);
    }

    #[test]
    fn test_build_pcap_multiple_packets() {
        let logs = vec![
            make_log(Some("call-1"), MessageDirection::Sent, "10.0.0.1:5060"),
            make_log(Some("call-1"), MessageDirection::Received, "10.0.0.1:5060"),
            make_log(Some("call-2"), MessageDirection::Sent, "10.0.0.2:5060"),
        ];
        let pcap = build_pcap(&logs);
        // Should have global header + 3 packets
        assert!(pcap.len() > 24 * 3);
    }

    #[test]
    fn test_build_pcap_filter_by_call_id() {
        let logs = vec![
            make_log(Some("call-A"), MessageDirection::Sent, "10.0.0.1:5060"),
            make_log(Some("call-B"), MessageDirection::Sent, "10.0.0.1:5060"),
            make_log(Some("call-A"), MessageDirection::Received, "10.0.0.1:5060"),
        ];

        // Filter to only call-A
        let filtered: Vec<_> = logs
            .into_iter()
            .filter(|l| l.call_id.as_deref() == Some("call-A"))
            .collect();
        assert_eq!(filtered.len(), 2);

        let pcap = build_pcap(&filtered);
        assert!(pcap.len() > 24);
    }

    #[test]
    fn test_base64_encode_basic() {
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }

    #[test]
    fn test_chrono_format() {
        let formatted = chrono_format(1710000000000);
        // Should produce HH:MM:SS.mmm format
        assert_eq!(formatted.len(), 12);
        assert!(formatted.contains(':'));
        assert!(formatted.contains('.'));
    }
}
