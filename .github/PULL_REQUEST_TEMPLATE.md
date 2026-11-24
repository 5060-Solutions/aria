## What and why

<!-- One paragraph. Link the issue if there is one: "Fixes #123". -->

## How it was tested

<!-- Tick what you ran. CI runs the first three on macOS, Linux and Windows. -->

- [ ] `pnpm check` (eslint + tsc) and `pnpm test`
- [ ] `cd src-tauri && cargo clippy -- -D warnings && cargo test --lib`
- [ ] Against a real PBX, if this touches SIP or media (`scripts/pbx-e2e.sh`, or a manual call: which PBX, which transport)
- [ ] Screenshots or a recording, if this changes the UI

## Checklist

- [ ] Conventional commit title, e.g. `fix(sip): …`, `feat(audio): …`, `ci: …`
- [ ] New UI strings exist in all four locales (`src/locales/{en,de,es,fr}.json`); the locale test enforces it
- [ ] Lint findings fixed in the code, not by changing the lint configuration
- [ ] Lockfile changes only move versions forward, and the description says what moved
