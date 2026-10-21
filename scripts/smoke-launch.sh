#!/usr/bin/env bash
#
# Launch the built Aria binary and fail if it crashes or panics during startup.
#
# Why this exists
# ---------------
# v0.5.0 shipped a binary that panicked on the first line a user hits:
#
#     thread 'main' panicked at src/sip/mod.rs:534:9:
#     there is no reactor running, must be called from the context of a Tokio
#     1.x runtime
#
# SipManager's constructor spawns Tokio tasks, but run() executed on a bare OS
# main thread with no runtime in context. Every test used #[tokio::test], which
# supplies a runtime, so nothing in the suite ever exercised the real startup
# path -- and CI *built* the bundle but never *launched* it. A green pipeline
# shipped a binary that could not open.
#
# This script closes that gap: it runs the actual built artifact and fails the
# release if the process dies or prints a panic during startup. The publish job
# depends on the build jobs, so a failure here keeps the broken build in the
# draft and off the updater. It catches this bug and any other startup crash.
#
# Usage: scripts/smoke-launch.sh <target-triple>
#   SMOKE_STARTUP_SECONDS  how long the app must stay up to pass (default 8)
#
set -euo pipefail

TARGET="${1:?usage: smoke-launch.sh <target-triple>}"
STARTUP_SECONDS="${SMOKE_STARTUP_SECONDS:-8}"
TARGET_DIR="${CARGO_TARGET_DIR:-src-tauri/target}"
UNAME_S="$(uname -s)"
LOG="$(mktemp)"

# Locate the executable the build produced for this target.
find_bin() {
  local rel="$TARGET_DIR/$TARGET/release"
  if [ "$UNAME_S" = "Darwin" ]; then
    # Launch the .app's inner executable so bundle resources resolve the same
    # way they do for a user double-clicking the app.
    local app
    app="$(find "$rel/bundle/macos" -maxdepth 1 -name '*.app' 2>/dev/null | head -1 || true)"
    if [ -n "$app" ]; then
      find "$app/Contents/MacOS" -maxdepth 1 -type f -perm -u+x 2>/dev/null | head -1
      return
    fi
  fi
  # Linux and the fallback: the raw release binary.
  find "$rel" -maxdepth 1 -type f -name 'aria' -perm -u+x 2>/dev/null | head -1
}

BIN="$(find_bin)"
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "smoke-launch: FAIL -- no executable found under $TARGET_DIR/$TARGET/release" >&2
  exit 1
fi
echo "smoke-launch: launching $BIN"
echo "smoke-launch: it must stay up for ${STARTUP_SECONDS}s without panicking"

# Start it in the background with stdio captured. On a headless Linux runner the
# webview needs a display; the startup panic fires before the window is created,
# but a clean start needs a display to survive the wait, so wrap in xvfb there.
if [ "$UNAME_S" = "Linux" ] && [ -z "${DISPLAY:-}" ]; then
  xvfb-run -a "$BIN" >"$LOG" 2>&1 &
else
  "$BIN" >"$LOG" 2>&1 &
fi
PID=$!

alive=1
for _ in $(seq 1 "$STARTUP_SECONDS"); do
  if ! kill -0 "$PID" 2>/dev/null; then
    alive=0
    break
  fi
  sleep 1
done

# Stop the app (and, on Linux, the xvfb wrapper, which takes its child with it).
kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true

echo "---- startup output ----"
cat "$LOG" || true
echo "------------------------"

if grep -qiE 'panicked|no reactor running' "$LOG"; then
  echo "smoke-launch: FAIL -- the app panicked during startup"
  exit 1
fi
if [ "$alive" -eq 0 ]; then
  echo "smoke-launch: FAIL -- the app exited during the ${STARTUP_SECONDS}s startup window"
  exit 1
fi

echo "smoke-launch: PASS -- the app started and stayed up"
