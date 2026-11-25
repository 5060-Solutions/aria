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
  <a href="https://github.com/5060-Solutions/aria/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/5060-Solutions/aria/ci.yml?branch=main&style=flat-square&label=CI" alt="CI" /></a>
  <a href="https://github.com/5060-Solutions/aria/security/code-scanning"><img src="https://img.shields.io/badge/CodeQL-enabled-2ea44f?style=flat-square" alt="CodeQL" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-non--commercial-blue?style=flat-square" alt="License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square" alt="Platform" />
</p>

<p align="center">
  <a href="https://aria-softphone.com"><strong>Download</strong></a> ·
  <a href="https://github.com/5060-Solutions/aria/releases">Releases</a> ·
  <a href="https://github.com/5060-Solutions/aria/issues/new?template=bug_report.yml">Report a bug</a> ·
  <a href="https://github.com/5060-Solutions/aria/discussions">Discussions</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
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
- Opus, G.711 (μ-law / A-law) and G.729 (8 kbps, for narrow links; Annex A, with Annex B comfort noise decoded)
- SRTP media encryption
- Echo cancellation, noise suppression and automatic gain control
- Choose input and output devices, and run a mic and speaker test

**Recording**
- Record any call to WAV with one click (⌘R)
- Per-account auto-record, off by default
- Recordings have their own place in the left bar, with transcripts beside them when transcription is on
- Optional on-device transcription and summaries of recorded calls: download the models in Settings, nothing is uploaded

**Everyday use**
- Contacts with favorites and search, plus macOS Contacts import
- Presence (BLF) for colleagues' extensions, shown live in Contacts
- Visual voicemail on an Aria PBX: see who called, play the message in the app, read its transcription, call back or delete. Other PBXes keep the one-click dial-in.
- Business texting (SMS and MMS) on an Aria PBX: shared inboxes for the numbers assigned to your extension, threads with delivery status, pictures, live updates and desktop notifications
- Call history with callback and CSV export
- Keyboard-driven: ⌘D dialer, ⌘↵ call / answer, ⌘K hang up, ⌘M mute, ⌘H hold, ⌘, settings
- Dark and light themes; English, Spanish, German and French
- Signed builds with in-app updates that never restart mid-call

**Setup and troubleshooting**
- Guided setup, or scan a QR code to provision an account
- Advanced options for PBXes that need them: separate auth username, auth-realm override, custom registrar
- Diagnostics window, docked beside the phone or floating: live SIP trace with millisecond timing, per-call filtering, RTP stats, and text, PCAP and report exports for your PBX admin

## Getting started

1. Download Aria from [aria-softphone.com](https://aria-softphone.com) or the
   [latest release](https://github.com/5060-Solutions/aria/releases/latest).
2. On first launch, enter your **server**, **username** and **password**. Leave
   domain blank unless your provider says otherwise. Prefer **TLS** if your
   provider supports it.
3. If registration fails with an authentication error on a FreeSWITCH-based
   PBX, open **Advanced** and set **Auth Realm** to the realm your provider
   gives you.

## Assistants (MCP)

Aria can run a small [MCP](https://modelcontextprotocol.io) server on your
computer so an AI assistant such as Claude Code or Claude Desktop can look at
the phone for you: which accounts are registered, which calls are up, and the
live SIP trace — enough to answer "why won't it register?" from the actual
messages rather than a guess. It is off by default, listens on `127.0.0.1`
only, and never hands out passwords: digest responses and SRTP keys are
redacted from every trace it returns.

Two switches in **Settings → Assistants (MCP)**:

- **Allow local assistants to read status and traces** turns the server on
  (default port 48060). This gives read-only tools: `get_status`,
  `list_accounts`, `list_calls`, `get_sip_trace`, `list_recordings` and
  `explain_last_failure`.
- **Allow assistants to place and control calls** adds `place_call`,
  `answer_call`, `hangup_call`, `send_dtmf`, `hold_call` and `transfer_call`.
  Even then, every `place_call` pops up a dialog in Aria — *"An assistant
  wants to call 1234 from Work"* — and nothing is dialled unless you click
  **Allow** within 30 seconds. Allowed and denied requests are both written
  to the Diagnostics panel.

Every request must carry a bearer token. Aria generates one the first time
you turn the server on and keeps it in the OS keychain; the Settings section
shows it masked with **Copy** and **Regenerate** buttons. The same section
gives you the one-line setup for Claude Code, ready to copy:

```bash
claude mcp add --transport http aria http://127.0.0.1:48060/mcp --header "Authorization: Bearer <token>"
```

The switches are stored in `mcp.json` in the app's data directory
(`{ "enabled": false, "port": 48060, "allow_call_control": false }`).

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
pnpm tauri build --features ai-summaries          # what releases ship (transcription + summaries)
```

The published installers are built with `ai-summaries`: transcription and
call summaries. Both stay off until you download a speech model (and, for
summaries, a summary model) in **Settings → Transcription** and turn the
switch on; the models are never part of the installer, and nothing leaves
your computer. A build made without the feature shows that section as
unavailable and points at the standard download.

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed.

## Testing

Every push and pull request runs the same checks on macOS, Linux and Windows
(`.github/workflows/ci.yml`): `eslint`, `tsc`, the Vitest suite, `cargo clippy
-- -D warnings` and `cargo test`. CodeQL scans the Rust, TypeScript and
workflow code weekly and on every PR, and Dependabot opens grouped update PRs
on Mondays.

What the tests cover, and where to add yours:

| Layer | Where | What it proves |
|-------|-------|----------------|
| SIP and media logic | `#[test]`s beside the code in `src-tauri/src/sip/` | Parsing, SDP negotiation, auth, transactions, registration and call state machines |
| Whole calls | `src-tauri/src/sip/voip_stack_harness/` | The real `SipManager` against a fake PBX over loopback: registration, codec negotiation, hold/mute, re-INVITEs, SRTP rekeys, retransmissions |
| MCP, provisioning, menus | `src-tauri/src/mcp/`, `provision/`, `menu.rs` | Tool permissions and consent, provisioning links, menu state |
| Frontend logic | `src/**/*.test.ts` (Vitest) | Pure utilities and store logic: SIP URIs, hints, codecs, provisioning flow, locale parity |
| Live PBX | `src-tauri/src/sip/pbx_e2e.rs`, `#[ignore]`d | Real calls through a real PBX, on demand (below) |

```bash
pnpm check && pnpm test                                   # frontend
cd src-tauri && cargo clippy -- -D warnings && cargo test --lib   # Rust, ~1 min warm
```

React components have no automated tests yet; UI changes are checked by hand
and with screenshots in the PR. Logic that needs testing is kept out of
components, in `src/utils` or the stores, where Vitest can reach it.

### End-to-end test against a PBX

`src-tauri/src/sip/pbx_e2e.rs` registers two real `SipManager`s on two
extensions of a live PBX and has one call the other through it: the inbound
INVITE is answered, RTP is checked in both directions, DTMF is sent, the
callee holds and resumes, the caller hangs up and the callee sees the BYE.
Companion tests cover an outbound call to voicemail, a blind transfer and an
SDES-SRTP offer. They are `#[ignore]`d — they need the network, audio devices
and two extensions (a third for the transfer: this PBX only transfers to a
registered extension) — so they run on demand, not in CI:

```bash
# existing extensions
ARIA_E2E_PBX_DOMAIN=test.ariaroute.com ARIA_E2E_PBX_SERVER=sip.ariaroute.com \
ARIA_E2E_EXT_A=1901 ARIA_E2E_PASS_A=… ARIA_E2E_EXT_B=1902 ARIA_E2E_PASS_B=… \
scripts/pbx-e2e.sh

# or let the script create and delete throwaway extensions
ARIA_API_KEY=… ARIA_TENANT=… scripts/pbx-e2e.sh
```

The header of `scripts/pbx-e2e.sh` lists every variable. A failing test prints
both endpoints' SIP traces. Extensions can also be created with the aria-pbx
MCP server's `create_extension` tool and removed with
`call_api extensions delete <id>`.

## Contributing

Bug reports and feature requests use the [issue templates](https://github.com/5060-Solutions/aria/issues/new/choose);
questions go to [Discussions](https://github.com/5060-Solutions/aria/discussions).
[CONTRIBUTING.md](CONTRIBUTING.md) has the house rules (conventional commits,
every string in four locales, lint fixed in code), [SECURITY.md](SECURITY.md)
says how to report a vulnerability privately, and the
[code of conduct](CODE_OF_CONDUCT.md) applies everywhere the project meets.

## License

Free for non-commercial use. Commercial use requires a license. See [LICENSE](LICENSE).
