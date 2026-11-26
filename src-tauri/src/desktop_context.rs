//! What this computer is doing, for the PBX's call handoff: whether the
//! screen is locked, how long the person has been idle, and a hash that says
//! which network this is.
//!
//! No raw identifiers leave the machine. The network is reduced to a SHA-256
//! of the user's extension id, its gateway and its subnet (the contract's
//! `aria-net-v1` formula); the same user on the same network gets the same
//! hash on every device, which is all the PBX needs to call two devices "near".

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::net::Ipv4Addr;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopContext {
    pub locked: bool,
    pub idle_secs: u64,
}

/// The network address of `ip` under `mask`.
fn subnet(ip: Ipv4Addr, mask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(ip) & u32::from(mask))
}

/// The call-move contract's `network_hash`, identical on every platform:
/// `lowercase_hex(sha256("aria-net-v1|" + extension_id + "|" + gateway + "|" + network/prefix))`.
/// The extension id (a lowercase UUID) salts it per user.
pub fn network_hash(extension_id: &str, ip: Ipv4Addr, mask: Ipv4Addr, gateway: Ipv4Addr) -> String {
    let input = format!(
        "aria-net-v1|{}|{gateway}|{}/{}",
        extension_id.to_ascii_lowercase(),
        subnet(ip, mask),
        mask.octets().iter().map(|b| b.count_ones()).sum::<u32>()
    );
    let digest = Sha256::digest(input.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The address the OS would use to reach the internet (no packet is sent).
fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("203.0.113.1:9").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}

/// A `0xffffff00`-style (macOS ifconfig) or dotted netmask.
#[cfg(any(target_os = "macos", test))]
fn parse_mask(s: &str) -> Option<Ipv4Addr> {
    if let Some(hex) = s.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).ok().map(Ipv4Addr::from);
    }
    s.parse().ok()
}

/// Gateway and netmask for `ip`, from the OS's own tools.
#[cfg(target_os = "macos")]
fn gateway_and_mask(ip: Ipv4Addr) -> Option<(Ipv4Addr, Ipv4Addr)> {
    let route = run("/sbin/route", &["-n", "get", "default"])?;
    let value = |text: &str, key: &str| {
        text.lines().find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().to_string()))
    };
    let gateway: Ipv4Addr = value(&route, "gateway:")?.parse().ok()?;
    let iface = value(&route, "interface:")?;
    let cfg = run("/sbin/ifconfig", &[&iface])?;
    let mask = cfg.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        (w.next() == Some("inet") && w.next() == Some(&ip.to_string()))
            .then(|| l.split_whitespace().skip_while(|w| *w != "netmask").nth(1).and_then(parse_mask))
            .flatten()
    })?;
    Some((gateway, mask))
}

#[cfg(target_os = "windows")]
fn gateway_and_mask(ip: Ipv4Addr) -> Option<(Ipv4Addr, Ipv4Addr)> {
    // `route print -4 0.0.0.0`: "0.0.0.0  0.0.0.0  <gateway>  <interface ip>  <metric>"
    let route = run("route", &["print", "-4", "0.0.0.0"])?;
    let gateway = route.lines().find_map(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        (w.len() >= 4 && w[0] == "0.0.0.0" && w[1] == "0.0.0.0" && w[3] == ip.to_string()).then(|| w[2].parse().ok()).flatten()
    })?;
    // The mask is not in that table; a /24 is the home and office norm.
    Some((gateway, Ipv4Addr::new(255, 255, 255, 0)))
}

#[cfg(target_os = "linux")]
fn gateway_and_mask(_ip: Ipv4Addr) -> Option<(Ipv4Addr, Ipv4Addr)> {
    let route = run("ip", &["-4", "route", "show", "default"])?;
    let gateway = route.split_whitespace().skip_while(|w| *w != "via").nth(1)?.parse().ok()?;
    Some((gateway, Ipv4Addr::new(255, 255, 255, 0)))
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The hash of this network, or `None` with no IPv4 default route.
pub fn network_fingerprint(extension_id: &str) -> Option<String> {
    let ip = local_ipv4()?;
    let (gateway, mask) = gateway_and_mask(ip)?;
    Some(network_hash(extension_id, ip, mask, gateway))
}

/// `HIDIdleTime` (nanoseconds) in `ioreg -c IOHIDSystem` output.
#[cfg(any(target_os = "macos", test))]
pub fn parse_hid_idle_secs(ioreg: &str) -> Option<u64> {
    let line = ioreg.lines().find(|l| l.contains("\"HIDIdleTime\""))?;
    let ns: u64 = line.rsplit('=').next()?.trim().parse().ok()?;
    Some(ns / 1_000_000_000)
}

/// Whether `ioreg -n Root -d1` says the session's screen is locked.
#[cfg(any(target_os = "macos", test))]
pub fn parse_screen_locked(ioreg: &str) -> bool {
    ioreg.lines().any(|l| l.contains("CGSSessionScreenIsLocked") && l.contains("Yes"))
        || ioreg.contains("\"CGSSessionScreenIsLocked\"=Yes")
}

#[cfg(target_os = "macos")]
pub fn context() -> DesktopContext {
    let idle = run("/usr/sbin/ioreg", &["-c", "IOHIDSystem", "-d", "4"]).and_then(|t| parse_hid_idle_secs(&t)).unwrap_or(0);
    let locked = run("/usr/sbin/ioreg", &["-n", "Root", "-d1", "-w0"]).is_some_and(|t| parse_screen_locked(&t));
    DesktopContext { locked, idle_secs: idle }
}

#[cfg(target_os = "windows")]
pub fn context() -> DesktopContext {
    #[repr(C)]
    struct LastInputInfo {
        size: u32,
        time: u32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(info: *mut LastInputInfo) -> i32;
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> isize;
        fn CloseDesktop(desktop: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount() -> u32;
    }
    const DESKTOP_SWITCHDESKTOP: u32 = 0x0100;
    // SAFETY: plain Win32 calls with a correctly sized, owned struct.
    unsafe {
        let mut info = LastInputInfo { size: std::mem::size_of::<LastInputInfo>() as u32, time: 0 };
        let idle = if GetLastInputInfo(&mut info) != 0 {
            u64::from(GetTickCount().wrapping_sub(info.time)) / 1000
        } else {
            0
        };
        // While the workstation is locked the input desktop is the secure one
        // and cannot be opened by this process.
        let desktop = OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP);
        let locked = desktop == 0;
        if !locked {
            CloseDesktop(desktop);
        }
        DesktopContext { locked, idle_secs: idle }
    }
}

#[cfg(target_os = "linux")]
pub fn context() -> DesktopContext {
    DesktopContext { locked: false, idle_secs: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT: &str = "ce0284f1-9c1e-420e-8bff-3b7b1f33cc25";

    /// The vector every app asserts: extension, gateway 192.168.1.1, network 192.168.1.0/24.
    #[test]
    fn the_hash_matches_the_shared_vector() {
        let h = network_hash(EXT, Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(255, 255, 255, 0), Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(h, "c220200c4e27272ef046b841eba83e316a8c004fd8c5a91289d6df1936ba90a3");
    }

    #[test]
    fn the_hash_is_of_the_subnet_and_gateway_not_the_host() {
        let gw = Ipv4Addr::new(192, 168, 1, 1);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        let a = network_hash(EXT, Ipv4Addr::new(192, 168, 1, 20), mask, gw);
        let b = network_hash(&EXT.to_uppercase(), Ipv4Addr::new(192, 168, 1, 99), mask, gw);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert!(!a.contains("192"), "no raw address in the hash text");
    }

    #[test]
    fn another_network_or_user_is_another_hash() {
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        let a = network_hash(EXT, Ipv4Addr::new(192, 168, 1, 20), mask, Ipv4Addr::new(192, 168, 1, 1));
        let b = network_hash(EXT, Ipv4Addr::new(10, 0, 0, 20), mask, Ipv4Addr::new(10, 0, 0, 1));
        let c = network_hash("someone-else", Ipv4Addr::new(192, 168, 1, 20), mask, Ipv4Addr::new(192, 168, 1, 1));
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn masks_parse_in_both_spellings() {
        assert_eq!(parse_mask("0xffffff00"), Some(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(parse_mask("255.255.0.0"), Some(Ipv4Addr::new(255, 255, 0, 0)));
        assert_eq!(parse_mask("nope"), None);
    }

    #[test]
    fn reads_macos_idle_time_and_lock() {
        assert_eq!(parse_hid_idle_secs("    | |   \"HIDIdleTime\" = 185000000000\n"), Some(185));
        assert_eq!(parse_hid_idle_secs("nothing"), None);
        assert!(parse_screen_locked("\"IOConsoleUsers\" = ({\"CGSSessionScreenIsLocked\"=Yes,\"x\"=1})"));
        assert!(!parse_screen_locked("\"IOConsoleUsers\" = ({\"x\"=1})"));
    }
}
