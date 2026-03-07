use serde::{Deserialize, Serialize};

/// The type of SIP event package for a subscription
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Dialog,
    Presence,
}

impl EventType {
    pub fn event_header(&self) -> &str {
        match self {
            EventType::Dialog => "dialog",
            EventType::Presence => "presence",
        }
    }

    pub fn accept_header(&self) -> &str {
        match self {
            EventType::Dialog => "application/dialog-info+xml",
            EventType::Presence => "application/pidf+xml",
        }
    }
}

/// State of an individual subscription
#[derive(Debug, Clone, PartialEq)]
pub enum SubscriptionState {
    Pending,
    Active,
    Terminated,
}

/// Represents an active SUBSCRIBE dialog
#[derive(Debug, Clone)]
pub struct Subscription {
    pub id: String,
    pub target_uri: String,
    pub event_type: EventType,
    pub state: SubscriptionState,
    pub expires: u32,
    pub cseq: u32,
    pub call_id: String,
    pub from_tag: String,
    pub to_tag: Option<String>,
}

/// Presence state derived from NOTIFY bodies
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PresenceState {
    Unknown,
    Available,
    Busy,
    Away,
    OnThePhone,
    Ringing,
    DoNotDisturb,
}

/// A single BLF entry, serializable for the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlfEntry {
    pub extension: String,
    pub state: PresenceState,
    pub display_name: Option<String>,
}

/// Parse dialog-info+xml body (RFC 4235) to extract dialog state.
///
/// Example body:
/// ```xml
/// <?xml version="1.0"?>
/// <dialog-info xmlns="urn:ietf:params:xml:ns:dialog-info" version="1" state="full" entity="sip:100@example.com">
///   <dialog id="1" direction="initiator">
///     <state>confirmed</state>
///   </dialog>
/// </dialog-info>
/// ```
pub fn parse_dialog_info_xml(body: &str) -> PresenceState {
    // Extract the dialog state element value
    // We do simple text parsing to avoid pulling in an XML crate
    let state_str = extract_xml_element(body, "state");

    match state_str.as_deref() {
        Some("trying") | Some("proceeding") => PresenceState::Busy,
        Some("early") => PresenceState::Ringing,
        Some("confirmed") => PresenceState::OnThePhone,
        Some("terminated") => PresenceState::Available,
        _ => {
            // If no <dialog> child elements, the user is available
            // Check for "<dialog " or "<dialog>" but not "<dialog-info"
            let has_dialog_element = body.contains("<dialog ") || body.contains("<dialog>");
            if !has_dialog_element {
                PresenceState::Available
            } else {
                PresenceState::Unknown
            }
        }
    }
}

/// Parse PIDF presence XML (RFC 3863) to extract basic status.
///
/// Example body:
/// ```xml
/// <?xml version="1.0"?>
/// <presence xmlns="urn:ietf:params:xml:ns:pidf" entity="sip:100@example.com">
///   <tuple id="1">
///     <status><basic>open</basic></status>
///     <note>Away</note>
///   </tuple>
/// </presence>
/// ```
pub fn parse_pidf_xml(body: &str) -> PresenceState {
    let basic = extract_xml_element(body, "basic");
    let note = extract_xml_element(body, "note");

    match basic.as_deref() {
        Some("open") => {
            // Check note for more specific state
            match note.as_deref().map(|s| s.to_lowercase()) {
                Some(ref n) if n.contains("away") => PresenceState::Away,
                Some(ref n) if n.contains("dnd") || n.contains("do not disturb") => {
                    PresenceState::DoNotDisturb
                }
                Some(ref n) if n.contains("busy") || n.contains("on the phone") => {
                    PresenceState::Busy
                }
                Some(ref n) if n.contains("ringing") => PresenceState::Ringing,
                _ => PresenceState::Available,
            }
        }
        Some("closed") => PresenceState::Away,
        _ => PresenceState::Unknown,
    }
}

/// Extract the text content of the first occurrence of an XML element.
/// This is a simple parser that avoids bringing in a full XML crate.
fn extract_xml_element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);

    let start_pos = xml.find(&open)?;
    // Skip past the opening tag (handle attributes)
    let after_open = &xml[start_pos + open.len()..];
    let content_start = after_open.find('>')?;
    let content = &after_open[content_start + 1..];
    let end_pos = content.find(&close)?;
    let value = content[..end_pos].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Extract the extension/user part from a SIP URI like "sip:100@example.com"
pub fn extract_extension_from_uri(uri: &str) -> String {
    let without_scheme = uri
        .strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))
        .unwrap_or(uri);
    without_scheme
        .split('@')
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dialog_confirmed() {
        let xml = r#"<?xml version="1.0"?>
<dialog-info xmlns="urn:ietf:params:xml:ns:dialog-info" version="1" state="full" entity="sip:100@example.com">
  <dialog id="1" direction="initiator">
    <state>confirmed</state>
  </dialog>
</dialog-info>"#;
        assert_eq!(parse_dialog_info_xml(xml), PresenceState::OnThePhone);
    }

    #[test]
    fn test_parse_dialog_terminated() {
        let xml = r#"<?xml version="1.0"?>
<dialog-info xmlns="urn:ietf:params:xml:ns:dialog-info" version="1" state="full" entity="sip:100@example.com">
  <dialog id="1">
    <state>terminated</state>
  </dialog>
</dialog-info>"#;
        assert_eq!(parse_dialog_info_xml(xml), PresenceState::Available);
    }

    #[test]
    fn test_parse_dialog_early() {
        let xml = r#"<dialog-info><dialog id="1"><state>early</state></dialog></dialog-info>"#;
        assert_eq!(parse_dialog_info_xml(xml), PresenceState::Ringing);
    }

    #[test]
    fn test_parse_dialog_no_dialogs() {
        let xml = r#"<dialog-info state="full" entity="sip:100@example.com"></dialog-info>"#;
        assert_eq!(parse_dialog_info_xml(xml), PresenceState::Available);
    }

    #[test]
    fn test_parse_pidf_open() {
        let xml =
            r#"<presence><tuple id="1"><status><basic>open</basic></status></tuple></presence>"#;
        assert_eq!(parse_pidf_xml(xml), PresenceState::Available);
    }

    #[test]
    fn test_parse_pidf_closed() {
        let xml =
            r#"<presence><tuple id="1"><status><basic>closed</basic></status></tuple></presence>"#;
        assert_eq!(parse_pidf_xml(xml), PresenceState::Away);
    }

    #[test]
    fn test_parse_pidf_open_with_away_note() {
        let xml = r#"<presence><tuple id="1"><status><basic>open</basic></status><note>Away</note></tuple></presence>"#;
        assert_eq!(parse_pidf_xml(xml), PresenceState::Away);
    }

    #[test]
    fn test_extract_extension() {
        assert_eq!(extract_extension_from_uri("sip:100@example.com"), "100");
        assert_eq!(extract_extension_from_uri("sips:200@example.com"), "200");
        assert_eq!(extract_extension_from_uri("100@example.com"), "100");
    }
}
