# Security

## Reporting a vulnerability

Please report security problems privately through GitHub's
[private vulnerability reporting](https://github.com/5060-Solutions/aria/security/advisories/new)
(**Security → Report a vulnerability** on the repository page). Do not open a
public issue or discussion for anything that could be a vulnerability.

Tell us what you found, how to reproduce it, and what you think the impact is.
A SIP trace or PCAP from **Settings → Diagnostics** helps when the problem is on
the wire. You will get an acknowledgement within three business days, and we
will keep you informed as we work on a fix. We credit reporters in the release
notes unless you ask us not to.

## Supported versions

Only the latest release is supported. Aria updates itself, and every release is
signed (Apple notarization on macOS, Authenticode on Windows, a Tauri updater
signature everywhere), so staying current is the one upgrade path.

## What is in scope

- The Aria desktop app: SIP signalling, media (RTP/SRTP), the auto-updater,
  credential storage, call recording and transcription.
- The local MCP server the app can run for assistants (off by default, loopback
  only, bearer token required).
- Provisioning links and QR codes the app accepts.

The ariaroute PBX and the aria-softphone.com website are separate projects;
please report problems with them through the same channel and we will route
them.

## How Aria protects you

- SIP passwords live in the OS keychain (macOS Keychain, Windows Credential
  Manager, Linux Secret Service), never in app storage.
- TLS for signalling and SDES-SRTP for media are a setting away, and the call
  screen shows a padlock only when the media really is encrypted.
- The Diagnostics log and anything the MCP server returns have digest responses
  and SRTP keys redacted.
- The updater verifies every download against a key that is not on the build
  machines, and never restarts the app during a call.
