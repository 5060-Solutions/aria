//! System contacts integration.
//!
//! Provides access to the native contacts database on supported platforms.
//! Currently macOS support is planned but requires complex Objective-C interop.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemContact {
    pub id: String,
    pub name: String,
    pub phone: Option<String>,
}

/// Fetch contacts from the system contacts database.
/// 
/// Currently returns an error on all platforms. Full macOS Contacts.framework
/// integration is planned for a future release.
#[cfg(target_os = "macos")]
pub fn fetch_contacts() -> Result<Vec<SystemContact>, String> {
    // The macOS Contacts framework requires complex Objective-C protocol handling
    // and entitlements. For now, we return a helpful error message.
    // 
    // Full implementation would require:
    // 1. com.apple.security.personal-information.addressbook entitlement
    // 2. NSContactsUsageDescription in Info.plist
    // 3. CNContactStore authorization flow
    // 4. ProtocolObject casting for CNKeyDescriptor
    Err("macOS Contacts access requires app entitlements and permissions. This feature will be available in a future release.".into())
}

#[cfg(not(target_os = "macos"))]
pub fn fetch_contacts() -> Result<Vec<SystemContact>, String> {
    Err("System contacts not supported on this platform".into())
}
