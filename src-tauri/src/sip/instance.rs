//! The install's stable `+sip.instance` identifier (RFC 5626 §4.1).
//!
//! A UUID generated once and kept in the app's data directory, so a restart
//! registers as the same instance and the registrar replaces the old binding
//! rather than adding a second one.

use std::path::Path;
use std::sync::OnceLock;

const FILE_NAME: &str = "sip-instance-id";

/// Read the id at `dir`/`sip-instance-id`, or create it. A file that does not
/// hold a UUID is replaced.
pub fn load_or_create(dir: &Path) -> Result<String, String> {
    let path = dir.join(FILE_NAME);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim();
        if uuid::Uuid::parse_str(text).is_ok() {
            return Ok(text.to_ascii_lowercase());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    std::fs::write(&path, &id).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(id)
}

/// This install's id. If the data directory cannot be used the id lasts for
/// the process only, which costs a stale binding until it expires, not a
/// registration.
#[cfg(not(test))]
pub fn get() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        crate::app_data_root()
            .ok_or_else(|| "no data directory".to_string())
            .and_then(|dir| load_or_create(&dir))
            .unwrap_or_else(|e| {
                log::warn!("SIP instance id is not persisted: {e}");
                uuid::Uuid::new_v4().to_string()
            })
    })
}

/// Tests never touch the real data directory.
#[cfg(test)]
pub fn get() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aria-instance-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_id_is_created_once_and_then_read_back() {
        let dir = temp_dir("stable");
        let a = load_or_create(&dir).unwrap();
        assert!(uuid::Uuid::parse_str(&a).is_ok());
        assert_eq!(load_or_create(&dir).unwrap(), a);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_damaged_file_is_replaced() {
        let dir = temp_dir("damaged");
        std::fs::write(dir.join(FILE_NAME), "not a uuid").unwrap();
        let id = load_or_create(&dir).unwrap();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(), id);
        std::fs::remove_dir_all(dir).ok();
    }
}
