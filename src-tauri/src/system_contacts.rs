//! System contacts integration.
//!
//! On macOS the native address book is read through Contacts.framework. The
//! OS shows its own permission prompt the first time; a refusal is reported
//! as [`ContactsErrorKind::Denied`] so the UI can send the user to System
//! Settings, since the prompt is never shown twice.

use serde::{Deserialize, Serialize};

/// One phone number of one person, as the frontend imports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemContact {
    /// Stable across syncs: the address book identifier plus the number's
    /// position, so a person with two numbers yields two contacts.
    pub id: String,
    pub name: String,
    pub phone: Option<String>,
    /// The number's label as the address book shows it ("mobile", "work").
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactsErrorKind {
    /// The user refused, now or earlier. macOS will not ask again.
    Denied,
    /// A device policy or parental controls block access.
    Restricted,
    /// Not macOS.
    Unsupported,
    /// Anything else, in the OS's words.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactsError {
    pub kind: ContactsErrorKind,
    pub message: String,
}

impl ContactsError {
    pub fn new(kind: ContactsErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

// The raw shape and `flatten` only have a producer on macOS; other
// platforms still compile and test them.
#[cfg(any(target_os = "macos", test))]
/// A person as the address book stores them, before flattening.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawContact {
    pub identifier: String,
    pub given_name: String,
    pub family_name: String,
    pub nickname: String,
    pub organization_name: String,
    pub phones: Vec<RawPhone>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPhone {
    pub label: Option<String>,
    pub number: String,
}

#[cfg(any(target_os = "macos", test))]
/// One contact per usable phone number, named the way Contacts.app names
/// people: "Given Family", else the organization, else the nickname, else
/// the number itself. Numbers with no digit and people with no usable
/// number are dropped.
pub fn flatten(raw: &[RawContact]) -> Vec<SystemContact> {
    let mut out = Vec::new();
    for person in raw {
        let full = squeeze(&format!("{} {}", person.given_name, person.family_name));
        let name = [full, squeeze(&person.organization_name), squeeze(&person.nickname)]
            .into_iter()
            .find(|n| !n.is_empty());
        let phones = person
            .phones
            .iter()
            .filter(|p| p.number.chars().any(|c| c.is_ascii_digit()));
        for (position, phone) in phones.enumerate() {
            let number = phone.number.trim().to_string();
            out.push(SystemContact {
                id: format!("{}:{}", person.identifier, position),
                name: name.clone().unwrap_or_else(|| number.clone()),
                phone: Some(number),
                label: phone.label.clone(),
            });
        }
    }
    out
}

#[cfg(any(target_os = "macos", test))]
fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Read the address book, asking the OS for permission first if it has not
/// been decided yet. Blocks while the permission prompt is up, so call it
/// from a blocking task, never from the UI thread.
#[cfg(target_os = "macos")]
pub fn fetch_contacts() -> Result<Vec<SystemContact>, ContactsError> {
    macos::ensure_access()?;
    macos::read_all().map(|raw| flatten(&raw))
}

#[cfg(not(target_os = "macos"))]
pub fn fetch_contacts() -> Result<Vec<SystemContact>, ContactsError> {
    Err(ContactsError::new(
        ContactsErrorKind::Unsupported,
        "System contacts are only available on macOS for now",
    ))
}

/// The System Settings pane where a refused permission can be turned on.
#[cfg(target_os = "macos")]
pub const PRIVACY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Contacts";

#[cfg(target_os = "macos")]
mod macos {
    use super::{ContactsError, ContactsErrorKind, RawContact, RawPhone};
    use objc2::rc::Retained;
use objc2::AllocAnyThread;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2_contacts::{
        CNAuthorizationStatus, CNContact, CNContactFamilyNameKey, CNContactFetchRequest,
        CNContactGivenNameKey, CNContactNicknameKey, CNContactOrganizationNameKey,
        CNContactPhoneNumbersKey, CNContactStore, CNEntityType, CNKeyDescriptor, CNLabeledValue,
        CNPhoneNumber,
    };
    use objc2_foundation::{NSArray, NSError};
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::sync::Mutex;

    fn denied(detail: Option<String>) -> ContactsError {
        let mut message = String::from("Aria is not allowed to read your contacts");
        if let Some(d) = detail.filter(|d| !d.is_empty()) {
            message.push_str(": ");
            message.push_str(&d);
        }
        ContactsError::new(ContactsErrorKind::Denied, message)
    }

    fn describe(error: Option<Retained<NSError>>) -> String {
        error
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_else(|| "unknown error".into())
    }

    /// Ok when the user has allowed access, asking them first if macOS has
    /// not recorded an answer yet.
    pub fn ensure_access() -> Result<(), ContactsError> {
        let status =
            unsafe { CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts) };
        match status {
            CNAuthorizationStatus::Authorized => return Ok(()),
            CNAuthorizationStatus::Denied => return Err(denied(None)),
            CNAuthorizationStatus::Restricted => {
                return Err(ContactsError::new(
                    ContactsErrorKind::Restricted,
                    "Contacts access is blocked on this Mac by a device policy",
                ))
            }
            // NotDetermined, or a newer status this build does not know:
            // ask, and let the answer decide.
            _ => {}
        }

        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let tx = Mutex::new(Some(tx));
        let handler = block2::RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let outcome = if granted.as_bool() {
                Ok(())
            } else {
                // SAFETY: the framework hands us either null or a live NSError
                // for the duration of this callback.
                let detail = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
                Err(detail.unwrap_or_default())
            };
            if let Some(tx) = tx.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = tx.send(outcome);
            }
        });
        // Kept alive until the answer arrives: the request belongs to it.
        let store = unsafe { CNContactStore::new() };
        unsafe { store.requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &handler) };
        match rx.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(detail)) => Err(denied(Some(detail))),
            Err(_) => Err(ContactsError::new(
                ContactsErrorKind::Failed,
                "the permission request never completed",
            )),
        }
    }

    /// Every unified contact with the name and phone fields filled in.
    pub fn read_all() -> Result<Vec<RawContact>, ContactsError> {
        // SAFETY: these are the framework's own constant key names.
        let keys: [&ProtocolObject<dyn CNKeyDescriptor>; 5] = unsafe {
            [
                ProtocolObject::from_ref(CNContactGivenNameKey),
                ProtocolObject::from_ref(CNContactFamilyNameKey),
                ProtocolObject::from_ref(CNContactNicknameKey),
                ProtocolObject::from_ref(CNContactOrganizationNameKey),
                ProtocolObject::from_ref(CNContactPhoneNumbersKey),
            ]
        };
        let keys = NSArray::from_slice(&keys);
        let request =
            unsafe { CNContactFetchRequest::initWithKeysToFetch(CNContactFetchRequest::alloc(), &keys) };
        let store = unsafe { CNContactStore::new() };

        let found = RefCell::new(Vec::new());
        let each = block2::StackBlock::new(|contact: NonNull<CNContact>, _stop: NonNull<Bool>| {
            // SAFETY: the framework passes a live contact for the duration of
            // this callback.
            let contact = unsafe { contact.as_ref() };
            found.borrow_mut().push(raw_from(contact));
        });
        let mut error: Option<Retained<NSError>> = None;
        let ok = unsafe {
            store.enumerateContactsWithFetchRequest_error_usingBlock(&request, Some(&mut error), &each)
        };
        if !ok {
            return Err(ContactsError::new(ContactsErrorKind::Failed, describe(error)));
        }
        Ok(found.into_inner())
    }

    fn raw_from(contact: &CNContact) -> RawContact {
        let phones = unsafe { contact.phoneNumbers() };
        let phones = phones
            .iter()
            .map(|labeled| {
                let label = unsafe { labeled.label() }
                    .map(|l| unsafe { CNLabeledValue::<CNPhoneNumber>::localizedStringForLabel(&l) }.to_string())
                    .filter(|l| !l.is_empty());
                let number = unsafe { labeled.value().stringValue() }.to_string();
                RawPhone { label, number }
            })
            .collect();
        RawContact {
            identifier: unsafe { contact.identifier() }.to_string(),
            given_name: unsafe { contact.givenName() }.to_string(),
            family_name: unsafe { contact.familyName() }.to_string(),
            nickname: unsafe { contact.nickname() }.to_string(),
            organization_name: unsafe { contact.organizationName() }.to_string(),
            phones,
        }
    }

    /// Reads the real address book. Ignored because it needs a Contacts
    /// permission grant on this Mac (the OS prompts on first run):
    /// `cargo test --lib system_contacts -- --ignored --nocapture`.
    #[cfg(test)]
    mod live {
        #[test]
        #[ignore = "needs a Contacts permission grant on this Mac"]
        fn reads_the_address_book() {
            let got = super::super::fetch_contacts();
            match &got {
                Ok(contacts) => println!("read {} phone numbers", contacts.len()),
                Err(e) => println!("contacts error: {e:?}"),
            }
            assert!(got.is_ok(), "{got:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(identifier: &str, given: &str, family: &str, phones: &[(&str, &str)]) -> RawContact {
        RawContact {
            identifier: identifier.into(),
            given_name: given.into(),
            family_name: family.into(),
            phones: phones
                .iter()
                .map(|(label, number)| RawPhone {
                    label: (!label.is_empty()).then(|| label.to_string()),
                    number: number.to_string(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_person_with_one_number_is_one_contact() {
        let got = flatten(&[person("ABC", "Franco", "Rossi", &[("mobile", "+39 333 1234567")])]);
        assert_eq!(
            got,
            vec![SystemContact {
                id: "ABC:0".into(),
                name: "Franco Rossi".into(),
                phone: Some("+39 333 1234567".into()),
                label: Some("mobile".into()),
            }]
        );
    }

    #[test]
    fn every_number_becomes_its_own_contact_with_a_position_in_the_id() {
        let got = flatten(&[person("ABC", "Franco", "Rossi", &[("mobile", "+39 333 1234567"), ("work", "+39 02 1234567")])]);
        assert_eq!(got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["ABC:0", "ABC:1"]);
        assert!(got.iter().all(|c| c.name == "Franco Rossi"));
        assert_eq!(got[1].label.as_deref(), Some("work"));
    }

    #[test]
    fn names_fall_back_to_organization_then_nickname_then_the_number() {
        let org = RawContact {
            organization_name: "Acme SpA".into(),
            nickname: "ac".into(),
            ..person("ORG", "", "", &[("", "0212345")])
        };
        let nick = RawContact { nickname: "Fra".into(), ..person("NICK", " ", "", &[("", "333")]) };
        let bare = person("BARE", "", "", &[("", "+1 555 0100")]);
        let names: Vec<String> = flatten(&[org, nick, bare]).into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Acme SpA", "Fra", "+1 555 0100"]);
    }

    #[test]
    fn a_name_is_trimmed_and_single_spaced() {
        let got = flatten(&[person("ID", "  Anna ", "", &[("", "1")])]);
        assert_eq!(got[0].name, "Anna");
        let got = flatten(&[person("ID", "Anna", "  de  Luca ", &[("", "1")])]);
        assert_eq!(got[0].name, "Anna de Luca");
    }

    #[test]
    fn numbers_without_digits_and_people_without_numbers_are_dropped() {
        let got = flatten(&[
            person("NONE", "No", "Phone", &[]),
            person("JUNK", "Only", "Junk", &[("", "n/a")]),
            person("MIX", "One", "Good", &[("", "ext."), ("home", "555 0100")]),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "MIX:0");
        assert_eq!(got[0].phone.as_deref(), Some("555 0100"));
    }

    #[test]
    fn errors_serialize_with_a_snake_case_kind_for_the_frontend() {
        let e = ContactsError::new(ContactsErrorKind::Denied, "no");
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"kind":"denied","message":"no"}"#);
    }
}
