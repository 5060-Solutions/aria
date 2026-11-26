//! Which PBX API each account's password may be sent to.
//!
//! Visual voicemail and texting sign in to a PBX API with the account's SIP
//! password from the keychain. The address used to ride along in every
//! command from the webview, so anything able to run script there could name
//! its own server and have the password sent to it. The backend now keeps the
//! address itself: set by the setup-link redeem (backend-originated, after the
//! user confirmed the link), or by `pbx_bind` from Settings, and a *change* of
//! an existing address needs a native confirmation the webview cannot answer.
//! Commands take only an account id and look the rest up here.

use crate::voicemail::{validate_api_url, Mailbox, VoicemailError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// `scheme://host[:port]/`, as `validate_api_url` reduces it.
    pub api_url: String,
    pub extension: String,
    pub domain: String,
}

/// What asking to bind an account comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Already bound to exactly this.
    Unchanged,
    /// Nothing bound yet: bind it.
    Add,
    /// Bound to something else: only with the user's say-so.
    NeedsConfirmation,
}

pub fn decide(existing: Option<&Binding>, wanted: &Binding) -> Decision {
    match existing {
        None => Decision::Add,
        Some(b) if b == wanted => Decision::Unchanged,
        Some(_) => Decision::NeedsConfirmation,
    }
}

pub struct PbxBindings {
    path: Option<PathBuf>,
    map: Mutex<HashMap<String, Binding>>,
}

impl PbxBindings {
    pub fn load(path: Option<PathBuf>) -> Self {
        let map = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            map: Mutex::new(map),
        }
    }

    pub fn get(&self, account_id: &str) -> Option<Binding> {
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account_id)
            .cloned()
    }

    /// Bind without asking: for the backend's own redeem, never for a request.
    pub fn set(&self, account_id: &str, binding: Binding) {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(account_id.to_string(), binding);
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match serde_json::to_string(&*map) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(path, json) {
                        log::warn!("[pbx] could not save the PBX bindings: {e}");
                    }
                }
                Err(e) => log::warn!("[pbx] could not save the PBX bindings: {e}"),
            }
        }
    }

    /// The mailbox a command may use for `account_id`: the stored address,
    /// whatever the caller said.
    pub fn resolve(&self, account_id: &str) -> Result<Mailbox, VoicemailError> {
        let b = self.get(account_id).ok_or(VoicemailError::NotBound)?;
        Ok(Mailbox {
            account_id: account_id.to_string(),
            api_url: b.api_url,
            extension: b.extension,
            domain: b.domain,
        })
    }
}

/// A binding from what Settings or a setup link holds, with the address
/// reduced to its origin.
pub fn binding_from(
    api_url: &str,
    extension: &str,
    domain: &str,
) -> Result<Binding, VoicemailError> {
    let url = validate_api_url(api_url, cfg!(debug_assertions))?;
    Ok(Binding {
        api_url: url.to_string(),
        extension: extension.to_string(),
        domain: domain.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(url: &str) -> Binding {
        binding_from(url, "1001", "t.example").unwrap()
    }

    #[test]
    fn a_new_binding_is_added_the_same_one_is_unchanged_a_different_one_needs_a_say_so() {
        assert_eq!(decide(None, &b("https://api.example.com")), Decision::Add);
        assert_eq!(
            decide(
                Some(&b("https://api.example.com")),
                &b("https://api.example.com/path")
            ),
            Decision::Unchanged
        );
        assert_eq!(
            decide(
                Some(&b("https://api.example.com")),
                &b("https://evil.example.net")
            ),
            Decision::NeedsConfirmation
        );
    }

    #[test]
    fn resolve_uses_the_stored_address_only() {
        let bindings = PbxBindings::load(None);
        assert!(matches!(
            bindings.resolve("a"),
            Err(VoicemailError::NotBound)
        ));
        bindings.set("a", b("https://api.example.com"));
        let m = bindings.resolve("a").unwrap();
        assert_eq!(m.api_url, "https://api.example.com/");
        assert_eq!(m.extension, "1001");
    }

    #[test]
    fn bindings_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("aria-bind-{}", uuid::Uuid::new_v4()));
        let path = dir.join("pbx_bindings.json");
        PbxBindings::load(Some(path.clone())).set("a", b("https://api.example.com"));
        assert_eq!(
            PbxBindings::load(Some(path)).get("a"),
            Some(b("https://api.example.com"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_non_https_address_is_not_bound() {
        assert!(binding_from("http://evil.example.com", "1", "d").is_err());
    }
}
