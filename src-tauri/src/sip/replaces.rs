//! `Replaces` (RFC 3891): the rules for an INVITE that asks this app to
//! replace one of its dialogs, kept pure so each can be tested.

/// A parsed `Replaces` header: the dialog it names, from the point of view of
/// the UA that receives the INVITE (so `to_tag` is this app's tag and
/// `from_tag` the remote party's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaces {
    pub call_id: String,
    pub to_tag: String,
    pub from_tag: String,
    /// `early-only`: replace the dialog only while it is still early.
    pub early_only: bool,
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Parse `call-id;to-tag=x;from-tag=y[;early-only]`. The Call-ID may be
/// percent-encoded (it often carries an `@`); both tags are required.
pub fn parse(value: &str) -> Option<Replaces> {
    let mut parts = value.split(';');
    let call_id = percent_decode(parts.next()?.trim())?;
    if call_id.is_empty() {
        return None;
    }
    let (mut to_tag, mut from_tag, mut early_only) = (None, None, false);
    for param in parts {
        let param = param.trim();
        let (name, val) = match param.split_once('=') {
            Some((n, v)) => (n.trim(), Some(v.trim())),
            None => (param, None),
        };
        match (name.to_ascii_lowercase().as_str(), val) {
            ("to-tag", Some(v)) if !v.is_empty() => to_tag = Some(v.to_string()),
            ("from-tag", Some(v)) if !v.is_empty() => from_tag = Some(v.to_string()),
            ("early-only", None) => early_only = true,
            _ => {}
        }
    }
    Some(Replaces {
        call_id,
        to_tag: to_tag?,
        from_tag: from_tag?,
        early_only,
    })
}

/// What this app knows of one of its dialogs.
#[derive(Debug, Clone)]
pub struct Dialog {
    pub call_id_header: String,
    /// This app's tag and the remote party's.
    pub local_tag: String,
    pub remote_tag: String,
    /// This app sent the INVITE.
    pub initiated_here: bool,
    /// Answered (confirmed), as opposed to still ringing (early).
    pub confirmed: bool,
    pub ended: bool,
}

/// What to do with an INVITE carrying `Replaces`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 400: the header does not parse.
    BadRequest,
    /// 481: no such dialog, or one this app may not replace.
    NoSuchDialog,
    /// 486: the dialog is confirmed and the header said `early-only`.
    Busy,
    /// 400: the INVITE names its own dialog.
    Loop,
    /// Accept: end the dialog at this index and answer the new INVITE.
    Replace(usize),
}

/// RFC 3891 section 3: a Replaces matching no dialog is 481; so is an early
/// dialog this app did not start (an unanswered inbound call); `early-only`
/// against a confirmed dialog is 486.
pub fn decide(header: &str, new_call_id: &str, dialogs: &[Dialog]) -> Outcome {
    let Some(r) = parse(header) else {
        return Outcome::BadRequest;
    };
    if r.call_id == new_call_id {
        return Outcome::Loop;
    }
    let found = dialogs.iter().position(|d| {
        !d.ended
            && d.call_id_header == r.call_id
            && d.local_tag == r.to_tag
            && d.remote_tag == r.from_tag
    });
    let Some(i) = found else {
        return Outcome::NoSuchDialog;
    };
    let d = &dialogs[i];
    if !d.confirmed && !d.initiated_here {
        return Outcome::NoSuchDialog;
    }
    if d.confirmed && r.early_only {
        return Outcome::Busy;
    }
    Outcome::Replace(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(confirmed: bool, initiated_here: bool) -> Dialog {
        Dialog {
            call_id_header: "abc@host".into(),
            local_tag: "mine".into(),
            remote_tag: "theirs".into(),
            initiated_here,
            confirmed,
            ended: false,
        }
    }
    const H: &str = "abc%40host;to-tag=mine;from-tag=theirs";

    #[test]
    fn parses_an_escaped_call_id_and_both_tags() {
        let r = parse(H).unwrap();
        assert_eq!(
            (r.call_id.as_str(), r.to_tag.as_str(), r.from_tag.as_str()),
            ("abc@host", "mine", "theirs")
        );
        assert!(!r.early_only);
    }

    #[test]
    fn parses_early_only_and_ignores_case() {
        assert!(
            parse("x;TO-TAG=a;From-Tag=b;early-only")
                .unwrap()
                .early_only
        );
    }

    #[test]
    fn a_header_missing_a_tag_or_with_bad_escapes_is_refused() {
        assert_eq!(parse("x;to-tag=a"), None);
        assert_eq!(parse("x;from-tag=a"), None);
        assert_eq!(parse(";to-tag=a;from-tag=b"), None);
        assert_eq!(parse("x%zz;to-tag=a;from-tag=b"), None);
        assert_eq!(decide("x;to-tag=a", "n", &[]), Outcome::BadRequest);
    }

    #[test]
    fn a_confirmed_dialog_is_replaced() {
        assert_eq!(
            decide(H, "new", &[dialog(true, false)]),
            Outcome::Replace(0)
        );
    }

    #[test]
    fn an_unknown_dialog_or_wrong_tags_is_481() {
        assert_eq!(decide(H, "new", &[]), Outcome::NoSuchDialog);
        assert_eq!(
            decide(
                "abc%40host;to-tag=theirs;from-tag=mine",
                "new",
                &[dialog(true, false)]
            ),
            Outcome::NoSuchDialog
        );
        assert_eq!(
            decide(
                "zzz;to-tag=mine;from-tag=theirs",
                "new",
                &[dialog(true, false)]
            ),
            Outcome::NoSuchDialog
        );
    }

    #[test]
    fn an_ended_dialog_is_481() {
        let mut d = dialog(true, false);
        d.ended = true;
        assert_eq!(decide(H, "new", &[d]), Outcome::NoSuchDialog);
    }

    #[test]
    fn an_early_dialog_we_did_not_start_is_481_but_one_we_did_is_replaced() {
        assert_eq!(
            decide(H, "new", &[dialog(false, false)]),
            Outcome::NoSuchDialog
        );
        assert_eq!(
            decide(H, "new", &[dialog(false, true)]),
            Outcome::Replace(0)
        );
    }

    #[test]
    fn early_only_against_a_confirmed_dialog_is_486() {
        let h = format!("{H};early-only");
        assert_eq!(decide(&h, "new", &[dialog(true, true)]), Outcome::Busy);
        assert_eq!(
            decide(&h, "new", &[dialog(false, true)]),
            Outcome::Replace(0)
        );
    }

    #[test]
    fn an_invite_cannot_replace_its_own_dialog() {
        assert_eq!(decide(H, "abc@host", &[dialog(true, false)]), Outcome::Loop);
    }
}
