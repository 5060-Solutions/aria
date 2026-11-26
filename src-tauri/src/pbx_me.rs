//! The signed-in user's own routes on the Aria PBX (`/api/v1/me/...`): calls
//! active on other devices, moving a call, the device list, proximity and
//! context reports, and the call-handoff settings.
//!
//! The session token stays in this process (see `messaging`), so the webview
//! asks for a route by name and path and this module refuses everything that
//! is not one of these.

use crate::messaging::{call, MessagingError};
use crate::voicemail::{route, Mailbox, VoicemailError, VoicemailState};
use serde_json::Value;

/// A request that passed the allowlist: the path as segments below `/api/v1`
/// and the one query pair allowed on it, if any.
#[derive(Debug, PartialEq, Eq)]
pub struct MeRoute {
    pub segments: Vec<String>,
    pub query: Option<(String, String)>,
}

fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

/// Whether `method` on `path` (with an optional `?sip_instance=`) is one of
/// the routes this module carries.
pub fn check(method: &str, path: &str) -> Result<MeRoute, MessagingError> {
    let refuse = || MessagingError {
        kind: "rejected",
        message: format!("{method} {path} is not a route the app uses."),
        status: None,
        field: None,
    };
    let (path_only, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    let parts: Vec<&str> = path_only.trim_matches('/').split('/').collect();
    let ok = match (method, parts.as_slice()) {
        ("GET", ["me", "calls", "active"]) => true,
        ("GET", ["me", "devices"]) => true,
        ("GET", ["me", "devices", "current"]) => true,
        ("POST", ["me", "devices", "current", "proximity" | "context"]) => true,
        ("GET", ["me", "devices", "current", "ble-token"]) => true,
        ("GET" | "PUT", ["me", "call-handoff"]) => true,
        ("POST", ["me", "calls", id, "move"]) => is_uuid(id),
        _ => false,
    };
    if !ok {
        return Err(refuse());
    }
    let query = match query {
        None => None,
        Some(q) => {
            let (k, v) = q.split_once('=').ok_or_else(refuse)?;
            let names_device =
                parts == ["me", "devices"] || parts.starts_with(&["me", "devices", "current"]);
            if k != "sip_instance" || !names_device {
                return Err(refuse());
            }
            let v = reqwest::Url::parse(&format!("http://x/?{k}={v}"))
                .ok()
                .and_then(|u| u.query_pairs().next().map(|(_, v)| v.into_owned()))
                .ok_or_else(refuse)?;
            Some((k.to_string(), v))
        }
    };
    Ok(MeRoute {
        segments: parts.iter().map(|s| s.to_string()).collect(),
        query,
    })
}

/// Send the request and return the JSON answer (`null` for an empty one).
pub async fn request(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, MessagingError> {
    let route_ = check(method, path)?;
    let method =
        reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| VoicemailError::Malformed)?;
    let bytes = call(state, mailbox, password, &|http, api, s| {
        let mut segments = vec!["api", "v1"];
        segments.extend(route_.segments.iter().map(String::as_str));
        let mut url = route(api, &segments);
        if let Some((k, v)) = &route_.query {
            url.query_pairs_mut().append_pair(k, v);
        }
        let req = http.request(method.clone(), url).bearer_auth(&s.jwt);
        match &body {
            Some(b) => req.json(b),
            None => req,
        }
    })
    .await?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|_| VoicemailError::Malformed.into())
}

/// The signed-in user's extension id (the salt of `network_hash`), from the
/// session, cached per account.
pub async fn extension_id(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
) -> Result<String, MessagingError> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = format!(
        "{}|{}|{}",
        mailbox.account_id, mailbox.extension, mailbox.domain
    );
    if let Some(id) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return Ok(id.clone());
    }
    let seen = std::sync::Mutex::new(String::new());
    call(state, mailbox, password, &|http, api, s| {
        *seen.lock().unwrap_or_else(|p| p.into_inner()) = s.extension_id.clone();
        http.get(route(api, &["api", "v1", "me", "devices"]))
            .bearer_auth(&s.jwt)
    })
    .await?;
    let id = seen.into_inner().unwrap_or_else(|p| p.into_inner());
    if id.is_empty() {
        return Err(VoicemailError::Malformed.into());
    }
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, id.clone());
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "6f1c1d3e-0a4b-4c55-9f0e-2d3b8a7c9e10";

    #[test]
    fn the_routes_of_the_contract_pass() {
        for (m, p) in [
            ("GET", "/me/calls/active"),
            ("GET", "/me/devices"),
            ("GET", "/me/devices/current"),
            ("POST", "/me/devices/current/proximity"),
            ("POST", "/me/devices/current/context"),
            ("GET", "/me/call-handoff"),
            ("PUT", "/me/call-handoff"),
        ] {
            assert!(check(m, p).is_ok(), "{m} {p}");
        }
        assert!(check("POST", &format!("/me/calls/{ID}/move")).is_ok());
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(check("GET", "/me/calls/active/../../auth").is_err());
        assert!(check("DELETE", "/me/devices").is_err());
        assert!(check("POST", "/me/calls/not-a-uuid/move").is_err());
        assert!(check("POST", &format!("/me/calls/{ID}/../x/move")).is_err());
        assert!(check("GET", "/tenants/t/extensions").is_err());
        assert!(check("GET", "/me/devices?x=1").is_err());
    }

    #[test]
    fn only_the_current_device_takes_a_sip_instance() {
        let r = check("GET", "/me/devices/current?sip_instance=urn:uuid:6f1c%3Aa").unwrap();
        assert_eq!(
            r.query,
            Some(("sip_instance".into(), "urn:uuid:6f1c:a".into()))
        );
        assert!(check("GET", "/me/devices?sip_instance=urn:uuid:x").is_ok());
        let r = check(
            "POST",
            "/me/devices/current/context?sip_instance=urn:uuid:x",
        )
        .unwrap();
        assert_eq!(r.query, Some(("sip_instance".into(), "urn:uuid:x".into())));
        assert!(check("GET", "/me/call-handoff?sip_instance=urn:uuid:x").is_err());
        assert!(check("GET", "/me/devices/current?other=1").is_err());
    }
}
