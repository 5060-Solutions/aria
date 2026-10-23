#!/usr/bin/env bash
#
# Launch the built Aria artifact and fail if it crashes or panics during
# startup, or never reports ready.
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
# release if the process dies, prints a panic, or does not reach the end of
# startup. The publish job depends on the build jobs, so a failure here keeps
# the broken build in the draft and off the updater.
#
# What "ready" means
# ------------------
# setup() in src-tauri/src/lib.rs logs "SIP event forwarding started" once the
# app is built and its event loop is running -- the last thing startup does.
# The script polls the captured output for that line and passes the moment it
# appears, so a healthy app is not held for a fixed sleep and a slow runner is
# not failed for being slow. It fails on a panic line, on the process exiting,
# or when the line has not appeared after SMOKE_TIMEOUT_SECONDS.
#
# What is launched
# ---------------
#   macOS    the executable inside bundle/macos/*.app
#   Linux    bundle/appimage/*.AppImage with --appimage-extract-and-run (no
#            FUSE on the runner); the raw release binary if there is none
#   Windows  release/aria.exe
#
# Usage: scripts/smoke-launch.sh <target-triple>
#   Pass "." for a local build without --target (target/release).
#   SMOKE_TIMEOUT_SECONDS  how long to wait for the ready line (default 60)
#
set -euo pipefail

TARGET="${1:?usage: smoke-launch.sh <target-triple>}"
TIMEOUT_SECONDS="${SMOKE_TIMEOUT_SECONDS:-60}"
TARGET_DIR="${CARGO_TARGET_DIR:-src-tauri/target}"
UNAME_S="$(uname -s)"
LOG="$(mktemp)"
READY_LINE='SIP event forwarding started'
PANIC_RE='panicked|no reactor running'

case "$UNAME_S" in
  MINGW*|MSYS*|CYGWIN*) OS=windows ;;
  Darwin) OS=macos ;;
  *) OS=linux ;;
esac

# The release directories a build may have landed in: the requested triple
# under the target dir, then the same without a triple (a build without
# --target). Nothing else: a fallback to some other directory could launch a
# stale build and pass for an artifact that was never checked.
release_dirs() {
  echo "$TARGET_DIR/$TARGET/release"
  echo "$TARGET_DIR/release"
}

# Every `find | head` here is guarded with `|| true`: under pipefail a missing
# directory makes the pipeline fail, and inside "$(...)" that would abort the
# script under `set -e` before the explicit "no executable found" check could
# say so.
find_bin() {
  local rel found app
  for rel in $(release_dirs); do
    [ -d "$rel" ] || continue
    case "$OS" in
      macos)
        # Launch the .app's inner executable so bundle resources resolve the
        # same way they do for a user double-clicking the app.
        app="$(find "$rel/bundle/macos" -maxdepth 1 -name '*.app' 2>/dev/null | head -1 || true)"
        if [ -n "$app" ]; then
          found="$(find "$app/Contents/MacOS" -maxdepth 1 -type f -perm -u+x 2>/dev/null | head -1 || true)"
          [ -n "$found" ] && { echo "$found"; return; }
        fi
        ;;
      linux)
        # The shipped artifact first; the raw binary only if there is none.
        found="$(find "$rel/bundle/appimage" -maxdepth 1 -name '*.AppImage' 2>/dev/null | head -1 || true)"
        [ -n "$found" ] && { echo "$found"; return; }
        found="$(find "$rel" -maxdepth 1 -type f -name 'aria' -perm -u+x 2>/dev/null | head -1 || true)"
        [ -n "$found" ] && { echo "$found"; return; }
        ;;
      windows)
        found="$(find "$rel" -maxdepth 1 -type f -iname 'aria.exe' 2>/dev/null | head -1 || true)"
        [ -n "$found" ] && { echo "$found"; return; }
        found="$(find "$rel" -maxdepth 1 -type f -iname '*.exe' 2>/dev/null | head -1 || true)"
        [ -n "$found" ] && { echo "$found"; return; }
        ;;
    esac
  done
}

BIN="$(find_bin || true)"
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "smoke-launch: FAIL -- no executable found under:" >&2
  release_dirs | sed 's/^/  /' >&2
  exit 1
fi

PID=""
XVFB_PID=""
SCRIPT_PGID="$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' ' || true)"

# Stop the app and everything it started. The app is launched as its own
# process group (job control is on for the launch), so killing the group
# takes the AppImage runtime, the extracted payload and WebKit's helper
# processes with it; killing only $PID used to leave the xvfb-run wrapper's
# children orphaned and running. The group is only used when it really is
# separate from this script's own, so the script never signals itself.
stop_app() {
  [ -n "$PID" ] || return 0
  local pgid=""
  if [ "$OS" != windows ]; then
    pgid="$(ps -o pgid= -p "$PID" 2>/dev/null | tr -d ' ' || true)"
  fi
  if [ -n "$pgid" ] && [ "$pgid" != "$SCRIPT_PGID" ]; then
    kill -TERM -- "-$pgid" 2>/dev/null || true
  else
    kill -TERM "$PID" 2>/dev/null || true
  fi
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$PID" 2>/dev/null || break
    sleep 0.5
  done
  if kill -0 "$PID" 2>/dev/null; then
    if [ -n "$pgid" ] && [ "$pgid" != "$SCRIPT_PGID" ]; then
      kill -KILL -- "-$pgid" 2>/dev/null || true
    elif [ "$OS" = windows ]; then
      # A native GUI process may ignore MSYS signals; take its tree down.
      taskkill //F //T //IM "$(basename "$BIN")" >/dev/null 2>&1 || true
    else
      kill -KILL "$PID" 2>/dev/null || true
    fi
  fi
  wait "$PID" 2>/dev/null || true
  if [ -n "$pgid" ] && [ "$pgid" != "$SCRIPT_PGID" ]; then
    # Nothing from the app's group may survive the run.
    local left
    left="$(pgrep -g "$pgid" 2>/dev/null || true)"
    if [ -n "$left" ]; then
      echo "smoke-launch: processes left in group $pgid, killing: $left"
      kill -KILL -- "-$pgid" 2>/dev/null || true
    fi
  fi
  PID=""
}

stop_xvfb() {
  [ -n "$XVFB_PID" ] || return 0
  kill -TERM "$XVFB_PID" 2>/dev/null || true
  wait "$XVFB_PID" 2>/dev/null || true
  XVFB_PID=""
}

cleanup() {
  stop_app
  stop_xvfb
}
trap cleanup EXIT

# On a headless Linux runner the webview needs a display. Start Xvfb directly
# rather than through xvfb-run: the wrapper shell was what `kill` used to hit,
# leaving Xvfb and the app behind. Pick a display that is not in use.
start_xvfb() {
  local n
  for n in 99 100 101 102 103; do
    [ -e "/tmp/.X${n}-lock" ] && continue
    Xvfb ":$n" -screen 0 1280x1024x24 -nolisten tcp >/dev/null 2>&1 &
    XVFB_PID=$!
    local i
    for i in $(seq 1 50); do
      if [ -S "/tmp/.X11-unix/X${n}" ]; then
        export DISPLAY=":$n"
        echo "smoke-launch: Xvfb on $DISPLAY (pid $XVFB_PID)"
        return 0
      fi
      kill -0 "$XVFB_PID" 2>/dev/null || break
      sleep 0.1
    done
    stop_xvfb
  done
  echo "smoke-launch: FAIL -- could not start Xvfb" >&2
  exit 1
}

echo "smoke-launch: launching $BIN"
echo "smoke-launch: waiting up to ${TIMEOUT_SECONDS}s for \"$READY_LINE\""

# The ready line is logged at info; make sure the environment cannot filter
# it out (the app's own default is info as well).
export RUST_LOG="${RUST_LOG:-info}"

if [ "$OS" = linux ] && [ -z "${DISPLAY:-}" ]; then
  start_xvfb
fi

# With job control on, a background job gets its own process group (pgid ==
# pid), which is what stop_app kills. Turned off again right after so the
# rest of the script behaves as a plain non-interactive shell.
set -m
if [[ "$BIN" == *.AppImage ]]; then
  "$BIN" --appimage-extract-and-run >"$LOG" 2>&1 &
else
  "$BIN" >"$LOG" 2>&1 &
fi
PID=$!
set +m

# Poll until the app is ready, dead, panicking or out of time.
verdict=timeout
deadline=$((SECONDS + TIMEOUT_SECONDS))
while :; do
  if grep -qiE "$PANIC_RE" "$LOG"; then
    verdict=panic
    break
  fi
  if grep -qF "$READY_LINE" "$LOG"; then
    verdict=ready
    break
  fi
  if ! kill -0 "$PID" 2>/dev/null; then
    # Let whatever it wrote on the way out land before classifying.
    sleep 1
    if grep -qiE "$PANIC_RE" "$LOG"; then verdict=panic; else verdict=exited; fi
    break
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    break
  fi
  sleep 0.5
done
elapsed=$((SECONDS - deadline + TIMEOUT_SECONDS))

# bash reports a signal-killed job on stderr ("Terminated: 15 ...") as it
# reaps it; that is the expected shutdown, not a failure, so keep it out of
# the output.
stop_app 2>/dev/null
stop_xvfb 2>/dev/null

echo "---- startup output ----"
cat "$LOG" || true
echo "------------------------"

case "$verdict" in
  ready)
    echo "smoke-launch: PASS -- ready after ${elapsed}s (\"$READY_LINE\")"
    ;;
  panic)
    echo "smoke-launch: FAIL -- the app panicked during startup"
    exit 1
    ;;
  exited)
    echo "smoke-launch: FAIL -- the app exited after ${elapsed}s before reporting ready"
    exit 1
    ;;
  *)
    echo "smoke-launch: FAIL -- no \"$READY_LINE\" within ${TIMEOUT_SECONDS}s"
    exit 1
    ;;
esac
