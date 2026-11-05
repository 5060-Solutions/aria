//! One-click setup links: `aria://provision?v=1&url=<redeem URL>`.
//!
//! The webview parses the `aria://` link and hands the redeem URL here; the
//! HTTP calls happen in Rust so the reply never passes through the webview's
//! network stack, and the password is written straight to the credential
//! store rather than returned. (Registering the line afterwards loads it into
//! the webview and back over IPC, as it does for every account at startup.)
//!
//! The flow is two calls against the same URL:
//! - `GET` returns a preview (tenant, extension, domain) and does not use the
//!   link up. The UI shows it and asks the user to confirm.
//! - `POST`, only after the user clicks Add, returns the credentials and uses
//!   the link up.
//!
//! Any web page can fire an `aria://` link, so nothing here runs without the
//! user's click on the confirm dialog, and the URL is re-checked here: the
//! webview is not trusted to have validated it. A redeem must return the line
//! the preview showed, and may only overwrite an account that the same host
//! set up earlier: a server cannot use a link to take over another line.
//!
//! Never log the URL (its last path segment is the one-time token), the
//! response body or the password.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;

/// Every redeem URL's path starts with this; the token follows.
const PATH_PREFIX: &str = "/provision/softphone/";
/// Bigger than any real reply by two orders of magnitude.
const MAX_BODY_BYTES: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

/// Why a setup link could not be used. `user_message` is what the UI shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionError {
    /// The URL failed validation; the reason is safe to log (no token).
    InvalidUrl(&'static str),
    /// 404: unknown, used or expired token.
    Expired,
    /// Any other non-200 status, including a redirect.
    Status(u16),
    /// The body is not JSON, or misses a required field, or a field is unusable.
    BadReply(&'static str),
    /// `v` is not 1.
    Version,
    TooLarge,
    Timeout,
    Unreachable,
    /// The redeem returned a different line than the preview showed.
    Mismatch,
    /// The id offered for a new account is already an existing account's.
    AccountIdInUse,
    /// The password could not be saved.
    Storage(String),
}

impl ProvisionError {
    pub fn user_message(&self) -> String {
        match self {
            Self::InvalidUrl(_) => "This setup link isn't valid.".into(),
            Self::Expired => {
                "This setup link has expired or was already used. Ask for a new one.".into()
            }
            Self::Status(code) => format!("The setup server returned an error (HTTP {code})."),
            Self::BadReply(_) | Self::TooLarge => {
                "The setup server sent a reply Aria can't read.".into()
            }
            Self::Version => "This setup link needs a newer version of Aria.".into(),
            Self::Timeout => "The setup server took too long to answer. Try again.".into(),
            Self::Unreachable => {
                "Couldn't reach the setup server. Check your connection and try again.".into()
            }
            Self::Mismatch => {
                "The setup server sent a different line than it showed. Nothing was changed.".into()
            }
            Self::AccountIdInUse => "Couldn't add the line. Try the link again.".into(),
            Self::Storage(e) => format!("Couldn't save the line's password: {e}"),
        }
    }
}

/// Check a redeem URL against the contract and return it parsed.
///
/// `https` only, except plain `http` to `localhost`/`127.0.0.1` when
/// `allow_local_http` (debug builds, for testing against a local stub). No
/// userinfo, and the path must be `/provision/softphone/<token>`.
pub fn validate_redeem_url(raw: &str, allow_local_http: bool) -> Result<reqwest::Url, ProvisionError> {
    let url = reqwest::Url::parse(raw).map_err(|_| ProvisionError::InvalidUrl("unparseable"))?;
    let host = url.host_str().ok_or(ProvisionError::InvalidUrl("no host"))?;
    match url.scheme() {
        "https" => {}
        "http" if allow_local_http && matches!(host, "localhost" | "127.0.0.1") => {}
        _ => return Err(ProvisionError::InvalidUrl("not https")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProvisionError::InvalidUrl("has userinfo"));
    }
    match url.path().strip_prefix(PATH_PREFIX) {
        Some(token) if !token.is_empty() => {}
        _ => return Err(ProvisionError::InvalidUrl("wrong path")),
    }
    Ok(url)
}

/// What the user is shown before they confirm: `host[:port]` of the server.
pub fn server_host(url: &reqwest::Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client, ProvisionError> {
    reqwest::Client::builder()
        // A redirect could send the request somewhere other than the host the
        // user was shown, so a 3xx is a failure like any other non-200.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .user_agent(concat!("Aria/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| ProvisionError::Unreachable)
}

/// Strip the URL from reqwest's error before it goes anywhere: its Display
/// would print the token.
fn net_error(e: reqwest::Error) -> ProvisionError {
    let e = e.without_url();
    if e.is_timeout() {
        ProvisionError::Timeout
    } else {
        log::warn!("[provision] request failed: {e}");
        ProvisionError::Unreachable
    }
}

/// Send the request and return the parsed JSON object of a 200 reply.
async fn fetch_json(
    request: reqwest::RequestBuilder,
    host: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ProvisionError> {
    let mut response = request
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(net_error)?;
    let status = response.status().as_u16();
    if status != 200 {
        log::warn!("[provision] {host} answered HTTP {status}");
        return Err(if status == 404 { ProvisionError::Expired } else { ProvisionError::Status(status) });
    }
    if response.content_length().is_some_and(|n| n > MAX_BODY_BYTES as u64) {
        return Err(ProvisionError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(net_error)? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            log::warn!("[provision] {host} sent more than {MAX_BODY_BYTES} bytes");
            return Err(ProvisionError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| ProvisionError::BadReply("not JSON"))?;
    let serde_json::Value::Object(map) = value else {
        return Err(ProvisionError::BadReply("not an object"));
    };
    if map.get("v").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(ProvisionError::Version);
    }
    Ok(map)
}

// ── Field checks ────────────────────────────────────────────────────────────
//
// The server is the one the user agreed to, but what it returns still ends up
// in SIP headers and on screen, so anything that could break either is refused.

fn get_str<'a>(map: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(serde_json::Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Display text: control characters, quotes, backslashes and angle brackets
/// dropped (a display name goes into a SIP quoted-string), length capped.
pub(crate) fn clean_text(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\\' | '<' | '>'))
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A DNS name or IPv4 address (no port).
fn is_host(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !s.starts_with(['.', '-'])
}

/// The user part of a SIP URI, without anything that would need escaping.
fn is_sip_user(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.!~*'()&=+$,;".contains(c))
}

fn host_field(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    what: &'static str,
) -> Result<String, ProvisionError> {
    let v = get_str(map, key).ok_or(ProvisionError::BadReply(what))?;
    if is_host(v) {
        Ok(v.to_ascii_lowercase())
    } else {
        Err(ProvisionError::BadReply(what))
    }
}

fn user_field(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    what: &'static str,
) -> Result<String, ProvisionError> {
    let v = get_str(map, key).ok_or(ProvisionError::BadReply(what))?;
    if is_sip_user(v) {
        Ok(v.to_string())
    } else {
        Err(ProvisionError::BadReply(what))
    }
}

// ── Preview ─────────────────────────────────────────────────────────────────

/// What the confirm dialog shows. Nothing secret.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionPreview {
    pub tenant_name: String,
    pub extension: String,
    pub display_name: String,
    pub domain: String,
    pub expires_at: Option<String>,
    /// The host the link points at, which is who is setting the line up.
    pub server_host: String,
}

pub async fn fetch_preview(url: &reqwest::Url, timeout: Duration) -> Result<ProvisionPreview, ProvisionError> {
    let host = server_host(url);
    let map = fetch_json(client(timeout)?.get(url.clone()), &host).await?;
    let preview = ProvisionPreview {
        tenant_name: get_str(&map, "tenant_name").map(|s| clean_text(s, 100)).unwrap_or_default(),
        extension: user_field(&map, "extension", "extension")?,
        display_name: get_str(&map, "display_name").map(|s| clean_text(s, 100)).unwrap_or_default(),
        domain: host_field(&map, "domain", "domain")?,
        expires_at: get_str(&map, "expires_at").map(|s| clean_text(s, 40)),
        server_host: host.clone(),
    };
    log::info!("[provision] preview from {host}");
    Ok(preview)
}

// ── Redeem ──────────────────────────────────────────────────────────────────

/// The credentials a redeem returns, before the password is put away.
#[derive(Clone, PartialEq)]
pub struct RedeemedLine {
    pub tenant_name: String,
    pub display_name: String,
    pub username: String,
    pub auth_username: Option<String>,
    pub password: String,
    pub domain: String,
    /// `host:port`, the form `AccountConfig::outbound_proxy` takes.
    pub outbound_proxy: Option<String>,
    /// The proxy's host, so the TLS certificate is checked against the host
    /// actually connected to. The registrar stays unset: REGISTER goes to the
    /// domain.
    pub tls_server_name: Option<String>,
    pub port: u16,
    pub transport: String,
    pub voicemail: Option<String>,
    /// The media encryption the server asks for: `"sdes"` or `"disabled"`;
    /// `None` when absent or unrecognised. The UI decides what to apply (SDES
    /// only over TLS).
    pub srtp: Option<String>,
}

// Hand-written so the password can never reach a log line through `{:?}`.
impl std::fmt::Debug for RedeemedLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedeemedLine")
            .field("username", &self.username)
            .field("domain", &self.domain)
            .field("outbound_proxy", &self.outbound_proxy)
            .field("port", &self.port)
            .field("transport", &self.transport)
            .field("srtp", &self.srtp)
            .finish_non_exhaustive()
    }
}

/// `sip.example.com`, `sip.example.com:5061` or `sips:sip.example.com:5061`
/// → host and the port, if one was given.
fn parse_proxy(raw: &str) -> Option<(String, Option<u16>)> {
    let lower = raw.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("sips:")
        .or_else(|| lower.strip_prefix("sip:"))
        .unwrap_or(&lower);
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => (h, Some(p.parse::<u16>().ok().filter(|p| *p != 0)?)),
        None => (rest, None),
    };
    is_host(host).then(|| (host.to_string(), port))
}

pub fn parse_redeem(map: &serde_json::Map<String, serde_json::Value>) -> Result<RedeemedLine, ProvisionError> {
    let username = user_field(map, "username", "username")?;
    let auth_username = match get_str(map, "auth_username") {
        Some(_) => Some(user_field(map, "auth_username", "auth_username")?),
        None => None,
    };
    let password = map
        .get("password")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
        .ok_or(ProvisionError::BadReply("password"))?
        .to_string();
    let domain = host_field(map, "domain", "domain")?;
    let transport = match get_str(map, "transport").map(str::to_ascii_lowercase) {
        None => "tls".to_string(),
        Some(t) if matches!(t.as_str(), "udp" | "tcp" | "tls") => t,
        Some(_) => return Err(ProvisionError::BadReply("transport")),
    };
    let port = match map.get("port") {
        None | Some(serde_json::Value::Null) => {
            if transport == "tls" { 5061 } else { 5060 }
        }
        Some(v) => v
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0)
            .ok_or(ProvisionError::BadReply("port"))?,
    };
    let (outbound_proxy, tls_server_name) = match get_str(map, "outbound_proxy") {
        None => (None, None),
        Some(raw) => {
            let (host, own_port) = parse_proxy(raw).ok_or(ProvisionError::BadReply("outbound_proxy"))?;
            (Some(format!("{host}:{}", own_port.unwrap_or(port))), Some(host))
        }
    };
    // Optional; anything but the two known values (a newer mode, a typo, a
    // non-string) counts as absent, which leaves SRTP as it is.
    let srtp = get_str(map, "srtp")
        .map(str::to_ascii_lowercase)
        .filter(|s| matches!(s.as_str(), "sdes" | "disabled"));
    let voicemail = get_str(map, "voicemail")
        .filter(|v| v.len() <= 32 && v.chars().all(|c| c.is_ascii_digit() || "*#+".contains(c)))
        .map(str::to_string);
    Ok(RedeemedLine {
        tenant_name: get_str(map, "tenant_name").map(|s| clean_text(s, 100)).unwrap_or_default(),
        display_name: get_str(map, "display_name")
            .map(|s| clean_text(s, 100))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| username.clone()),
        username,
        auth_username,
        password,
        domain,
        outbound_proxy,
        tls_server_name,
        port,
        transport,
        voicemail,
        srtp,
    })
}

pub async fn redeem(url: &reqwest::Url, timeout: Duration) -> Result<RedeemedLine, ProvisionError> {
    let host = server_host(url);
    let map = fetch_json(client(timeout)?.post(url.clone()), &host).await?;
    let line = parse_redeem(&map)?;
    log::info!("[provision] redeemed at {host}: {:?}", line);
    if line.srtp.as_deref() == Some("sdes") && line.transport != "tls" {
        // SDES puts the media keys in the SDP, which would travel in clear.
        log::warn!("[provision] setup server asked for SDES-SRTP over {}; not enabling it without TLS", line.transport);
    }
    Ok(line)
}

// ── Commands ────────────────────────────────────────────────────────────────

/// An account the app already has, so a link for the same line can update it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownAccount {
    pub id: String,
    pub username: String,
    pub domain: String,
    /// The setup-link host that created the account; `None` if set up by hand.
    #[serde(default)]
    pub provisioned_from: Option<String>,
}

/// The line the user agreed to in the preview. A redeem for any other line is
/// refused.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedLine {
    pub extension: String,
    pub domain: String,
}

/// What a redeem does to the app's accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LineMode {
    /// A new account.
    Added,
    /// The account this same host set up earlier, updated in place.
    Updated,
    /// A new account, although one with the same username@domain exists: that
    /// one was set up by hand or by another host, and is left alone.
    Separate,
}

/// What the UI gets back after a redeem: everything but the password, which
/// is already in the credential store under `account_id`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedLine {
    pub account_id: String,
    pub mode: LineMode,
    /// The setup link's `host[:port]`, recorded on the account.
    pub provisioned_from: String,
    pub tenant_name: String,
    pub display_name: String,
    pub username: String,
    pub auth_username: Option<String>,
    pub domain: String,
    pub outbound_proxy: Option<String>,
    pub tls_server_name: Option<String>,
    pub port: u16,
    pub transport: String,
    pub voicemail: Option<String>,
    /// `"sdes"`, `"disabled"`, or `None` (absent or unrecognised).
    pub srtp: Option<String>,
}

/// The account a redeemed line goes to. Only an account the same host set up
/// may be overwritten; one with the same identity from anywhere else gets a
/// separate new account beside it.
pub fn pick_account(line: &RedeemedLine, known: &[KnownAccount], host: &str, new_id: &str) -> (String, LineMode) {
    let same_identity: Vec<&KnownAccount> = known
        .iter()
        .filter(|a| a.username == line.username && a.domain.trim().eq_ignore_ascii_case(&line.domain))
        .collect();
    if let Some(own) = same_identity
        .iter()
        .find(|a| a.provisioned_from.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(host)))
    {
        return (own.id.clone(), LineMode::Updated);
    }
    let mode = if same_identity.is_empty() { LineMode::Added } else { LineMode::Separate };
    (new_id.to_string(), mode)
}

fn is_account_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A password on its way to the credential store. Never printed.
pub struct Password(pub String);

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(..)")
    }
}

/// Redeem and check the reply against the preview, choosing the account it
/// goes to. Writes nothing: the caller stores the password only on `Ok`, so a
/// refused redeem leaves every account and keychain entry as it was.
pub async fn redeem_checked(
    url: &reqwest::Url,
    expected: &ExpectedLine,
    known: &[KnownAccount],
    new_account_id: &str,
    timeout: Duration,
) -> Result<(ProvisionedLine, Password), ProvisionError> {
    // A "new" id that is already an account's would overwrite that account's
    // password. Checked before the link is used up.
    if known.iter().any(|a| a.id == new_account_id) {
        return Err(ProvisionError::AccountIdInUse);
    }
    let line = redeem(url, timeout).await?;
    if line.username != expected.extension || !line.domain.eq_ignore_ascii_case(expected.domain.trim()) {
        // The line's identity stays out of the log, like everything else the
        // server sent.
        log::warn!("[provision] redeem returned a different line than the preview; refused");
        return Err(ProvisionError::Mismatch);
    }
    let host = server_host(url);
    let (account_id, mode) = pick_account(&line, known, &host, new_account_id);
    let password = Password(line.password);
    Ok((
        ProvisionedLine {
            account_id,
            mode,
            provisioned_from: host,
            tenant_name: line.tenant_name,
            display_name: line.display_name,
            username: line.username,
            auth_username: line.auth_username,
            domain: line.domain,
            outbound_proxy: line.outbound_proxy,
            tls_server_name: line.tls_server_name,
            port: line.port,
            transport: line.transport,
            voicemail: line.voicemail,
            srtp: line.srtp,
        },
        password,
    ))
}

/// Bring the main window forward so the confirm dialog is seen. The OS
/// usually does this for a deep link already; not on every platform. Only
/// called for a link that passed validation.
fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn checked_url(url: &str) -> Result<reqwest::Url, String> {
    validate_redeem_url(url, cfg!(debug_assertions)).map_err(|e| {
        log::warn!("[provision] refused a setup link: {e:?}");
        e.user_message()
    })
}

/// Fetch a setup link's preview. Uses nothing up.
#[tauri::command]
pub async fn provision_link_preview(app: tauri::AppHandle, url: String) -> Result<ProvisionPreview, String> {
    let url = checked_url(&url)?;
    focus_main_window(&app);
    fetch_preview(&url, TIMEOUT).await.map_err(|e| e.user_message())
}

/// Redeem a setup link — only ever called from the confirm dialog's Add
/// button. Saves the password to the credential store and returns the rest.
#[tauri::command]
pub async fn provision_link_redeem(
    url: String,
    expected: ExpectedLine,
    accounts: Vec<KnownAccount>,
    new_account_id: String,
) -> Result<ProvisionedLine, String> {
    let url = checked_url(&url)?;
    if !is_account_id(&new_account_id) {
        return Err("Invalid account id".into());
    }
    let (line, password) = redeem_checked(&url, &expected, &accounts, &new_account_id, TIMEOUT)
        .await
        .map_err(|e| e.user_message())?;
    // The keychain can block on an unlock prompt; keep it off the runtime.
    let id = line.account_id.clone();
    tokio::task::spawn_blocking(move || crate::commands::store_credential_blocking(&id, &password.0))
        .await
        .map_err(|e| ProvisionError::Storage(e.to_string()).user_message())?
        .map_err(|e| ProvisionError::Storage(e).user_message())?;
    Ok(line)
}

#[cfg(test)]
mod tests;
