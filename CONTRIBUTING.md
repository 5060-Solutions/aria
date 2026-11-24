# Contributing to Aria

Thank you for helping. This page is the short version of how work lands here;
the [README](README.md) has the build instructions.

## Before you start

- **Bugs:** open a [bug report](https://github.com/5060-Solutions/aria/issues/new?template=bug_report.yml)
  first, with the SIP trace from **Settings → Diagnostics** when it is a
  signalling or media problem. A trace usually settles what the fix is.
- **Features:** open a [feature request](https://github.com/5060-Solutions/aria/issues/new?template=feature_request.yml)
  or start a [discussion](https://github.com/5060-Solutions/aria/discussions)
  so we can agree on the shape before you build it.
- **Questions:** Discussions, not issues.

## Setting up

Follow **Development** in the README: Node.js, pnpm, Rust stable, `meson` and
`ninja` for the echo canceller, and your platform's build tools. Then:

```bash
pnpm install
pnpm tauri:dev           # dev build with file-based credentials
```

## Making a change

1. Branch from `main`: `fix/short-name`, `feat/short-name`, `ci/…`, `docs/…`.
2. Write the test first when there is something to test. Pure logic belongs in
   `src/utils` or `src/types` with a `*.test.ts` next to it; SIP and media
   behaviour belongs in `src-tauri/src/sip` with a `#[test]`, or in the
   loopback harness (`voip_stack_harness`) when it needs a whole call against
   a fake PBX.
3. Run what CI runs, on your machine:

   ```bash
   pnpm check && pnpm test
   cd src-tauri && cargo clippy -- -D warnings && cargo test --lib
   ```

4. If you touched SIP or media, make a real call. `scripts/pbx-e2e.sh` runs
   the end-to-end tests against a PBX; its header lists the variables.
5. Open a pull request. The template asks how you tested it; answer it.

## House rules

- **Conventional commits:** `fix(sip): …`, `feat(audio): …`, `ci: …`,
  `docs: …`, `test(e2e): …`. The subject says what changed; the body says why.
  No sign-offs, trailers or tool attributions.
- **Every user-visible string is translated.** Add it to all four locales in
  `src/locales` (`en`, `de`, `es`, `fr`). The locale test fails on a missing
  key or an unused one.
- **Lint rules are not negotiable.** Fix the code. Do not change
  `eslint.config.js`, `tsconfig.json` or clippy settings to get green.
- **Lockfiles only move forward.** If `pnpm-lock.yaml` or `Cargo.lock`
  changes, say in the PR what moved and why. Dependabot opens weekly PRs for
  routine bumps; majors get their own PR with the migration notes.
- **Don't ship the dev feature.** `dev-insecure` stores credentials in a
  plain file for local builds only.
- **Keep secrets out.** No keys, tokens, real extensions or passwords in
  code, tests, fixtures or traces.

## Releases

Maintainers cut releases from a `release/x.y.z` branch that bumps the version
in the four places that carry it; pushing the `vx.y.z` tag runs
`.github/workflows/release.yml`, which builds, signs and publishes installers
for all platforms together with the updater manifest.

## License

Aria is free for non-commercial use and licensed for commercial use; see
[LICENSE](LICENSE). By contributing you agree that your contribution is
provided under the same terms.
