//! Emergency numbers, which are dialled whatever the registration state is.

/// The numbers treated as emergency calls: North America, the EU and much of
/// the world, the UK and others, and Australia.
const EMERGENCY_NUMBERS: &[&str] = &["911", "112", "999", "000"];

/// Whether a dial string or SIP URI is an emergency number: `911`,
/// `sip:911@example.com`, `tel:112`.
pub fn is_emergency(target: &str) -> bool {
    let t = target.trim();
    let t = ["sips:", "sip:", "tel:"]
        .iter()
        .find_map(|p| t.strip_prefix(p))
        .unwrap_or(t);
    let user = t.split(['@', ';']).next().unwrap_or("");
    EMERGENCY_NUMBERS.contains(&user)
}

#[cfg(test)]
mod tests {
    use super::is_emergency;

    #[test]
    fn recognises_numbers_in_every_form() {
        for t in [
            "911",
            " 112 ",
            "sip:911@pbx.example",
            "sips:999@pbx.example;user=phone",
            "tel:000",
        ] {
            assert!(is_emergency(t), "{t}");
        }
        for t in [
            "9111",
            "1911",
            "sip:1001@pbx.example",
            "+1911",
            "",
            "sip:x@911",
        ] {
            assert!(!is_emergency(t), "{t}");
        }
    }
}
