#!/usr/bin/env bash
#
# Verify a release's latest.json before it is published: every platform URL
# must download, and every download must pass the updater's minisign check
# against the public key in src-tauri/tauri.conf.json.
#
# Why this exists
# ---------------
# v0.6.9 (and earlier releases since Windows signing was added in March 2026)
# shipped a latest.json whose Windows entries were dead.
# The release job Authenticode-signed the .exe/.msi *after* tauri-action had
# uploaded them and written latest.json, then re-uploaded them with
# `gh release upload --clobber`. Clobbering deletes the asset and creates a new
# one with a new ID, so the manifest's Windows URLs returned 404; and the
# updater signature in the manifest covered the bytes from before Authenticode
# signing, so it would not have verified anyway. Every installed Windows client
# silently stopped receiving updates. macOS and Linux were fine, and nothing
# checked the manifest, so nobody noticed.
#
# Usage: scripts/verify-updater-manifest.sh <tag> [repo]
# Needs: gh (authenticated; the release may still be a draft), jq, minisign.

set -euo pipefail

tag="${1:?usage: $0 <tag> [owner/repo]}"
repo="${2:-${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY or pass owner/repo}}"
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

jq -r '.plugins.updater.pubkey' "$root/src-tauri/tauri.conf.json" | base64 -d > "$work/pubkey"

gh release download "$tag" --repo "$repo" --pattern latest.json --dir "$work"

expected_version="${tag#v}"
version="$(jq -r '.version' "$work/latest.json")"
if [ "$version" != "$expected_version" ]; then
  echo "latest.json version $version does not match tag $tag" >&2
  exit 1
fi

failures=0
declare -A checked=()
while IFS=$'\t' read -r platform url signature; do
  if [ -n "${checked[$url]:-}" ]; then
    echo "ok   $platform (same artifact as ${checked[$url]})"
    continue
  fi
  file="$work/artifact"
  # Draft assets are only reachable through the API with a token, which is
  # also how the updater URL (api.github.com/.../releases/assets/<id>) is
  # shaped, so fetch it the same way.
  if ! gh api -H 'Accept: application/octet-stream' "${url#https://api.github.com/}" > "$file" 2> "$work/err"; then
    echo "FAIL $platform: $url does not download: $(head -c 300 "$work/err")" >&2
    failures=$((failures + 1))
    continue
  fi
  printf '%s' "$signature" | base64 -d > "$work/sig"
  if minisign -V -q -p "$work/pubkey" -m "$file" -x "$work/sig"; then
    echo "ok   $platform ($(wc -c < "$file") bytes)"
    checked[$url]="$platform"
  else
    echo "FAIL $platform: signature does not verify for $url" >&2
    failures=$((failures + 1))
  fi
done < <(jq -r '.platforms | to_entries[] | [.key, .value.url, .value.signature] | @tsv' "$work/latest.json")

for required in windows-x86_64 darwin-aarch64 darwin-x86_64 linux-x86_64; do
  if ! jq -e --arg p "$required" '.platforms[$p]' "$work/latest.json" > /dev/null; then
    echo "FAIL latest.json has no $required entry" >&2
    failures=$((failures + 1))
  fi
done

if [ "$failures" -gt 0 ]; then
  echo "$failures updater manifest problem(s) in $tag" >&2
  exit 1
fi
echo "latest.json for $tag verified"
