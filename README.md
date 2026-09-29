<p align="center">
  <img src="public/icon.png" alt="Aria" width="128" height="128" />
</p>

<h1 align="center">Aria</h1>

<p align="center">
  <strong>A fast, native SIP softphone for macOS, Windows and Linux.</strong><br/>
  <em>by <a href="https://5060solutions.com">5060 Solutions</a></em>
</p>

<p align="center">
  <a href="https://github.com/5060-Solutions/aria/releases/latest"><img src="https://img.shields.io/github/v/release/5060-Solutions/aria?style=flat-square" alt="Release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-non--commercial-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square" alt="Platform" />
</p>

<p align="center">
  <a href="https://aria-softphone.com"><strong>Download</strong></a> ·
  <a href="https://github.com/5060-Solutions/aria/releases">Releases</a> ·
  <a href="https://github.com/5060-Solutions/aria/issues">Report a bug</a>
</p>

---

Aria connects to any standard SIP PBX or provider. It's built on Tauri and a
Rust SIP and media stack, not Electron and WebRTC, so it starts instantly and
stays light.

## Features

**Calling**
- Register one or more SIP accounts over UDP, TCP or TLS, and switch between them
- Place, answer, hold, mute and hang up; DTMF via RFC 2833
- Put a call on hold, take a second one and swap between them, or merge them into a three-way conference
- Click `tel:` and `sip:` links anywhere on your system to dial

**Audio**
- Opus and G.711 (μ-law / A-law)
- SRTP media encryption
- Echo cancellation, noise suppression and automatic gain control
- Choose input and output devices, and run a mic and speaker test

**Recording**
- Record any call to WAV with one click (⌘R)
- Per-account auto-record, off by default
- Recordings are linked from call history

**Everyday use**
- Contacts with favorites and search, plus macOS Contacts import
- Call history with callback and CSV export
- Keyboard-driven: ⌘D dialer, ⌘↵ call / answer, ⌘K hang up, ⌘M mute, ⌘H hold, ⌘, settings
- Dark and light themes; English, Spanish, German and French
- Signed builds with in-app updates that never restart mid-call

**Setup and troubleshooting**
- Guided setup, or scan a QR code to provision an account
- Advanced options for PBXes that need them: separate auth username, auth-realm override, custom registrar
- Diagnostics panel: live SIP message log, RTP stats and PCAP export for your PBX admin

## Getting started

1. Download Aria from [aria-softphone.com](https://aria-softphone.com) or the
   [latest release](https://github.com/5060-Solutions/aria/releases/latest).
2. On first launch, enter your **server**, **username** and **password**. Leave
   domain blank unless your provider says otherwise. Prefer **TLS** if your
   provider supports it.
3. If registration fails with an authentication error on a FreeSWITCH-based
   PBX, open **Advanced** and set **Auth Realm** to the realm your provider
   gives you.

## How it's built

```
React + MUI           UI
Tauri v2 IPC          bridge
Rust SipManager       registration, call control, presence
sip-5060 · rtp-engine SIP signalling · RTP/SRTP, codecs, audio I/O
```

The SIP stack is pure Rust, with no PJSIP or other C SIP library. Passwords
are stored in the OS keychain (macOS Keychain, Windows Credential Manager,
Linux Secret Service), never in app storage.

| Path | What's there |
|------|--------------|
| `src/` | React frontend: components, Zustand store, `useSip` IPC bindings |
| `src-tauri/src/commands.rs` | Every Tauri command the frontend can call |
| `src-tauri/src/sip/` | `SipManager`, transports, auth, request and response handlers |
| `src-tauri/src/ai.rs` | Optional on-device transcription (`ai` feature) |

## Development

**Prerequisites:** Node.js 20+, pnpm 9+, Rust stable, and `meson` + `ninja`
(for the echo canceller). You'll also need platform build tools:

- **macOS:** Xcode Command Line Tools
- **Windows:** Visual Studio Build Tools with the C++ workload. Build from a
  developer prompt, or meson picks up mingw's `g++` and the build fails.
- **Linux:** `build-essential`, `libwebkit2gtk-4.1-dev`, `libasound2-dev`

```bash
pnpm install
pnpm tauri:dev     # dev build
pnpm check         # eslint + tsc
cd src-tauri && cargo test --lib
```

`tauri:dev` enables the `dev-insecure` feature, which stores credentials in a
plain JSON file so you aren't prompted by the keychain on every rebuild. Never
ship a build with it enabled.

Styling and CSP behave differently in dev: `tauri dev` serves the UI from Vite,
so anything that depends on the bundled asset pipeline (for example the
Content Security Policy) only shows up in a real build. Check those with
`pnpm tauri build`.

```bash
pnpm tauri build                                  # current platform
pnpm tauri build --target universal-apple-darwin  # macOS universal
pnpm tauri build --features ai                    # with on-device transcription
```

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed.

## License

Free for non-commercial use. Commercial use requires a license. See [LICENSE](LICENSE).
