//! A local MCP (Model Context Protocol) server inside the app.
//!
//! Lets an assistant on this computer (Claude Code, Claude Desktop, …) read
//! Aria's registration state, calls and live SIP trace, and — only when the
//! user has turned that on, and with a dialog for each call — place and
//! control calls.
//!
//! * Off by default. `<app_data_dir>/mcp.json` holds the switches.
//! * Served over MCP Streamable HTTP on `127.0.0.1` only, path `/mcp`.
//! * Every request needs `Authorization: Bearer <token>`. The token is
//!   generated on first enable and kept in the OS keychain; it is never
//!   logged.
//!
//! `tools` holds the tools, `consent` the per-call permission flow. This
//! module owns the config, the token and the server task.

pub mod consent;
pub mod tools;

#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use base64::Engine;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tauri::{Emitter, State as TauriState};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::sip::SipManager;
use consent::{ConsentRegistry, Decision};
use tools::{AriaMcp, CallRequest, CallRequestSink, ToolContext, ToolSettings};

/// The port the server listens on unless the user picks another.
pub const DEFAULT_PORT: u16 = 48060;
/// Where the MCP endpoint lives on that port.
pub const MCP_PATH: &str = "/mcp";
const CONFIG_FILE: &str = "mcp.json";

// ── Config ──────────────────────────────────────────────────────────────────

/// The on-disk switches. Everything is off until the user turns it on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct McpConfig {
    /// Serve at all. Read-only tools work when this is on.
    pub enabled: bool,
    pub port: u16,
    /// Let assistants place and control calls (each new call still asks).
    pub allow_call_control: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            allow_call_control: false,
        }
    }
}

impl McpConfig {
    /// Read the config, or the defaults when the file is missing or broken.
    /// A broken file must not leave the server on by accident, and the
    /// defaults are all off.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("Ignoring unreadable {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }

    /// A port the listener can be given from Settings. 0 would pick a random
    /// port, which the setup line could never name.
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("Port must be between 1 and 65535".to_string());
        }
        Ok(())
    }
}

// ── Token ───────────────────────────────────────────────────────────────────

#[cfg(not(feature = "dev-insecure"))]
const KEYRING_SERVICE: &str = "aria-mcp";
#[cfg(not(feature = "dev-insecure"))]
const KEYRING_USER: &str = "token";

/// A fresh bearer token: 32 random bytes, base64url without padding.
pub fn generate_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Where the token lives: the OS keychain, or — like SIP passwords — a
/// plain file in the data directory under the `dev-insecure` feature so a
/// dev build is not met by a keychain prompt on every launch.
#[derive(Clone)]
pub struct TokenStore {
    #[cfg_attr(not(feature = "dev-insecure"), allow(dead_code))]
    data_dir: PathBuf,
}

impl TokenStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    #[cfg(feature = "dev-insecure")]
    fn insecure_path(&self) -> PathBuf {
        self.data_dir.join(".dev-mcp-token")
    }

    fn read_blocking(&self) -> Result<Option<String>, String> {
        #[cfg(feature = "dev-insecure")]
        {
            match std::fs::read_to_string(self.insecure_path()) {
                Ok(t) if !t.trim().is_empty() => Ok(Some(t.trim().to_string())),
                Ok(_) => Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("Could not read MCP token file: {e}")),
            }
        }
        #[cfg(not(feature = "dev-insecure"))]
        {
            let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
                .map_err(|e| format!("Keychain unavailable: {e}"))?;
            match entry.get_password() {
                Ok(t) => Ok(Some(t)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(format!("Could not read the MCP token from the keychain: {e}")),
            }
        }
    }

    fn write_blocking(&self, token: &str) -> Result<(), String> {
        #[cfg(feature = "dev-insecure")]
        {
            log::warn!("Storing the MCP token in a plain file (dev-insecure build)");
            std::fs::create_dir_all(&self.data_dir)
                .map_err(|e| format!("Could not create data dir: {e}"))?;
            std::fs::write(self.insecure_path(), token)
                .map_err(|e| format!("Could not write MCP token file: {e}"))
        }
        #[cfg(not(feature = "dev-insecure"))]
        {
            keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
                .map_err(|e| format!("Keychain unavailable: {e}"))?
                .set_password(token)
                .map_err(|e| format!("Could not store the MCP token in the keychain: {e}"))
        }
    }

    /// The keychain can block on an unlock prompt, so never on an async
    /// worker.
    pub async fn read(&self) -> Result<Option<String>, String> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.read_blocking())
            .await
            .map_err(|e| format!("keychain task failed: {e}"))?
    }

    pub async fn write(&self, token: String) -> Result<(), String> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.write_blocking(&token))
            .await
            .map_err(|e| format!("keychain task failed: {e}"))?
    }

    /// The stored token, or a new one stored now.
    pub async fn ensure(&self) -> Result<String, String> {
        if let Some(token) = self.read().await? {
            return Ok(token);
        }
        let token = generate_token();
        self.write(token.clone()).await?;
        Ok(token)
    }
}

// ── HTTP server ─────────────────────────────────────────────────────────────

/// The bearer token the middleware checks against. Shared so rotating it
/// takes effect without restarting the listener.
pub type SharedToken = Arc<RwLock<String>>;

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reject anything without the right `Authorization: Bearer` header. The
/// token itself is never written to the log, whichever way this goes.
async fn require_bearer(State(token): State<SharedToken>, req: Request, next: Next) -> Response {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let ok = match presented {
        Some(p) => constant_time_eq(p.as_bytes(), token.read().await.as_bytes()),
        None => false,
    };
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"aria-mcp\"")],
            "Unauthorized: this endpoint needs the Aria MCP bearer token",
        )
            .into_response();
    }
    next.run(req).await
}

/// A running listener. Dropping it does not stop the task; call `stop`.
pub struct ServerHandle {
    pub addr: SocketAddr,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ServerHandle {
    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = self.task.await;
        log::info!("MCP server on {} stopped", self.addr);
    }
}

/// Bind `bind` and serve MCP at `/mcp`, guarded by `token`.
///
/// Returns once the listener is bound, so a port already in use is reported
/// to the caller instead of to the log alone. Port 0 is allowed here (tests
/// use it); `addr` on the handle carries the real one.
pub async fn start_server(
    bind: SocketAddr,
    token: SharedToken,
    ctx: ToolContext,
) -> Result<ServerHandle, String> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("Could not listen on {bind}: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("Could not read the listener address: {e}"))?;

    let cancel = CancellationToken::new();
    let service = StreamableHttpService::new(
        move || Ok(AriaMcp::new(ctx.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            // One JSON reply per request, no server-side sessions: nothing
            // for a crashed client to leak, and the simplest thing for a
            // local assistant to talk to.
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_cancellation_token(cancel.clone()),
    );
    let app = Router::new()
        .route_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(token, require_bearer));

    let shutdown = cancel.clone();
    let task = tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await;
        if let Err(e) = result {
            log::error!("MCP server on {addr} failed: {e}");
        }
    });
    log::info!("MCP server listening on http://{addr}{MCP_PATH}");
    Ok(ServerHandle { addr, cancel, task })
}

// ── App state and commands ──────────────────────────────────────────────────

/// Shows a call request to the user by way of the webview.
struct TauriRequestSink {
    app: tauri::AppHandle,
}

impl CallRequestSink for TauriRequestSink {
    fn request(&self, request: CallRequest) {
        let payload = serde_json::json!({
            "requestId": request.request_id,
            "number": request.number,
            "accountId": request.account_id,
        });
        if let Err(e) = self.app.emit("mcp-call-request", payload) {
            log::error!("Could not show the MCP call request: {e}");
        }
    }
}

/// What Tauri manages: the config, the running server (if any) and what the
/// tools share with the Settings commands.
pub struct McpState {
    config_path: PathBuf,
    config: RwLock<McpConfig>,
    settings: Arc<RwLock<ToolSettings>>,
    token_store: TokenStore,
    token: SharedToken,
    consent: Arc<ConsentRegistry>,
    manager: SipManager,
    app: tauri::AppHandle,
    server: Mutex<Option<ServerHandle>>,
}

impl McpState {
    pub fn new(app: tauri::AppHandle, manager: SipManager, data_dir: PathBuf) -> Self {
        let config_path = data_dir.join(CONFIG_FILE);
        let config = McpConfig::load(&config_path);
        Self {
            settings: Arc::new(RwLock::new(ToolSettings {
                allow_call_control: config.allow_call_control,
            })),
            config: RwLock::new(config),
            config_path,
            token_store: TokenStore::new(data_dir),
            token: Arc::new(RwLock::new(String::new())),
            consent: Arc::new(ConsentRegistry::new()),
            manager,
            app,
            server: Mutex::new(None),
        }
    }

    fn tool_context(&self) -> ToolContext {
        ToolContext {
            manager: self.manager.clone(),
            consent: self.consent.clone(),
            settings: self.settings.clone(),
            requests: Arc::new(TauriRequestSink {
                app: self.app.clone(),
            }),
        }
    }

    /// At launch: bring the server up if the user left it on.
    pub async fn start_if_enabled(&self) {
        let config = self.config.read().await.clone();
        if !config.enabled {
            return;
        }
        if let Err(e) = self.restart(&config).await {
            log::error!("MCP server not started: {e}");
        }
    }

    /// Stop whatever is running and, if `config.enabled`, start on its port.
    async fn restart(&self, config: &McpConfig) -> Result<(), String> {
        let mut server = self.server.lock().await;
        if let Some(running) = server.take() {
            running.stop().await;
        }
        if !config.enabled {
            return Ok(());
        }
        config.validate()?;
        let token = self.token_store.ensure().await?;
        *self.token.write().await = token;
        let bind = SocketAddr::from(([127, 0, 0, 1], config.port));
        *server = Some(start_server(bind, self.token.clone(), self.tool_context()).await?);
        Ok(())
    }

    async fn status(&self) -> McpStatus {
        let config = self.config.read().await.clone();
        let running = self.server.lock().await.is_some();
        McpStatus {
            enabled: config.enabled,
            port: config.port,
            allow_call_control: config.allow_call_control,
            running,
        }
    }

    /// Apply new switches: persist them, hand `allow_call_control` to the
    /// tools, and restart the listener when enabled or port changed. If the
    /// port cannot be bound, the switch goes back off so Settings never
    /// shows a server that is not there.
    pub async fn apply(&self, mut next: McpConfig) -> Result<McpStatus, String> {
        next.validate()?;
        let previous = self.config.read().await.clone();
        self.settings.write().await.allow_call_control = next.allow_call_control;
        let running = self.server.lock().await.is_some();
        let needs_restart = next.enabled != previous.enabled
            || next.port != previous.port
            || (next.enabled && !running);
        let mut outcome = Ok(());
        if needs_restart {
            outcome = self.restart(&next).await;
            if outcome.is_err() {
                next.enabled = false;
                // Not an "unregister" of call control: the user's choice
                // stands for when the port is free again.
            }
        }
        *self.config.write().await = next.clone();
        next.save(&self.config_path)?;
        outcome?;
        Ok(self.status().await)
    }

    pub async fn token(&self) -> Result<Option<String>, String> {
        self.token_store.read().await
    }

    /// Replace the token. Takes effect immediately on the running server.
    pub async fn rotate_token(&self) -> Result<String, String> {
        let token = generate_token();
        self.token_store.write(token.clone()).await?;
        *self.token.write().await = token.clone();
        log::info!("MCP bearer token rotated");
        Ok(token)
    }

    pub fn decide(&self, request_id: &str, decision: Decision) -> Result<(), String> {
        self.consent.decide(request_id, decision)
    }
}

/// What Settings shows. `running` can be false with `enabled` true only in
/// the moment between a failed bind and the config being switched back.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub enabled: bool,
    pub port: u16,
    pub allow_call_control: bool,
    pub running: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSettingsInput {
    pub enabled: bool,
    pub port: u16,
    pub allow_call_control: bool,
}

#[tauri::command]
pub async fn mcp_get_config(state: TauriState<'_, McpState>) -> Result<McpStatus, String> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn mcp_set_config(
    config: McpSettingsInput,
    state: TauriState<'_, McpState>,
) -> Result<McpStatus, String> {
    state
        .apply(McpConfig {
            enabled: config.enabled,
            port: config.port,
            allow_call_control: config.allow_call_control,
        })
        .await
}

/// The bearer token, or `None` if the server has never been enabled.
#[tauri::command]
pub async fn mcp_get_token(state: TauriState<'_, McpState>) -> Result<Option<String>, String> {
    state.token().await
}

#[tauri::command]
pub async fn mcp_rotate_token(state: TauriState<'_, McpState>) -> Result<String, String> {
    state.rotate_token().await
}

/// The user's answer to an `mcp-call-request`. On allow, `call_id` is the
/// call the frontend placed; on failure to dial, `error` says why.
#[tauri::command]
pub async fn mcp_call_decision(
    request_id: String,
    allow: bool,
    call_id: Option<String>,
    error: Option<String>,
    state: TauriState<'_, McpState>,
) -> Result<(), String> {
    let decision = match (allow, call_id, error) {
        (true, Some(call_id), _) => Decision::allowed(call_id),
        (true, None, Some(error)) => Decision::denied(error),
        (true, None, None) => Decision::denied("the call could not be placed"),
        (false, _, _) => Decision::denied("denied"),
    };
    state.decide(&request_id, decision)
}
