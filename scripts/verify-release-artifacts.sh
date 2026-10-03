#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: verify-release-artifacts.sh <target> <dmg> <archive> <signature>

Verifies the architecture, deep code signature, launch behavior, and updater
archive for one macOS release build.
EOF
  exit 2
}

[[ $# -eq 4 ]] || usage

TARGET=$1
DMG=$2
ARCHIVE=$3
SIGNATURE=$4

case "$TARGET" in
  aarch64-apple-darwin)
    EXPECTED_ARCH=arm64
    ;;
  x86_64-apple-darwin)
    EXPECTED_ARCH=x86_64
    ;;
  *)
    echo "Unsupported macOS target: $TARGET" >&2
    exit 1
    ;;
esac

for path in "$DMG" "$ARCHIVE" "$SIGNATURE"; do
  [[ -f "$path" ]] || { echo "Missing release artifact: $path" >&2; exit 1; }
  [[ -s "$path" ]] || { echo "Empty release artifact: $path" >&2; exit 1; }
done

WORK_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/codetally-release-verify.XXXXXX")
MOUNT_POINT="$WORK_ROOT/dmg-mount"
mkdir -p "$MOUNT_POINT"
SMOKE_PID=

cleanup() {
  set +e
  if [[ -n "${SMOKE_PID:-}" ]] && kill -0 "$SMOKE_PID" 2>/dev/null; then
    kill "$SMOKE_PID" 2>/dev/null
    wait "$SMOKE_PID" 2>/dev/null
  fi
  if [[ -n "${MOUNT_POINT:-}" ]] && mount | grep -Fq " on $MOUNT_POINT "; then
    hdiutil detach "$MOUNT_POINT" -quiet -force
  fi
  rm -rf "$WORK_ROOT"
}
trap cleanup EXIT INT TERM

normalise_arches() {
  printf '%s' "$1" | tr -s '[:space:]' ' ' | sed 's/^ //;s/ $//'
}

verify_architecture() {
  local app=$1
  local plist="$app/Contents/Info.plist"
  [[ -f "$plist" ]] || { echo "Missing app Info.plist: $app" >&2; exit 1; }

  local executable_name
  executable_name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist" 2>/dev/null || true)
  [[ -n "$executable_name" ]] || { echo "Missing CFBundleExecutable: $app" >&2; exit 1; }

  local binary="$app/Contents/MacOS/$executable_name"
  [[ -x "$binary" ]] || { echo "Missing executable: $binary" >&2; exit 1; }

  otool -L "$binary" >/dev/null
  local actual_arches
  actual_arches=$(normalise_arches "$(lipo -archs "$binary")")
  [[ "$actual_arches" == "$EXPECTED_ARCH" ]] || {
    echo "Expected $EXPECTED_ARCH executable, found $actual_arches in $binary" >&2
    exit 1
  }
  printf '%s\n' "$binary"
}

verify_signed_app() {
  local app=$1
  echo "Verifying signed app: $app"
  codesign --verify --deep --strict --verbose=2 "$app"
  verify_architecture "$app" >/dev/null
}

find_single_app() {
  local root=$1
  local count
  count=$(find "$root" -type d -name 'CodeTally.app' -prune -print | wc -l | tr -d ' ')
  [[ "$count" == 1 ]] || {
    echo "Expected one CodeTally.app below $root, found $count" >&2
    exit 1
  }
  find "$root" -type d -name 'CodeTally.app' -prune -print -quit
}

launch_smoke_check() {
  local app=$1
  local binary
  binary=$(verify_architecture "$app")
  local log="$WORK_ROOT/codetally-smoke.log"
  local host_arch
  host_arch=$(uname -m)
  if [[ "$host_arch" != "$EXPECTED_ARCH" ]]; then
    echo "Skipping live launch smoke check for $EXPECTED_ARCH on $host_arch runner; signature, loadability, and architecture checks passed."
    return
  fi

  echo "Launching $binary for smoke check"
  arch -"$EXPECTED_ARCH" "$binary" >"$log" 2>&1 &
  SMOKE_PID=$!
  sleep 5

  if ! kill -0 "$SMOKE_PID" 2>/dev/null; then
    local exit_code
    if wait "$SMOKE_PID"; then exit_code=0; else exit_code=$?; fi
    echo "CodeTally exited during its launch smoke check (status $exit_code)." >&2
    sed -n '1,80p' "$log" >&2 || true
    exit 1
  fi

  kill "$SMOKE_PID"
  local attempt
  for attempt in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$SMOKE_PID" 2>/dev/null || break
    sleep 0.25
  done
  if kill -0 "$SMOKE_PID" 2>/dev/null; then
    kill -9 "$SMOKE_PID" 2>/dev/null || true
  fi
  wait "$SMOKE_PID" 2>/dev/null || true
  SMOKE_PID=
}

echo "Checking DMG integrity and mounting $DMG"
hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT_POINT" "$DMG"
DMG_APP=$(find_single_app "$MOUNT_POINT")
verify_signed_app "$DMG_APP"
launch_smoke_check "$DMG_APP"

echo "Detaching verified DMG"
hdiutil detach "$MOUNT_POINT" -quiet
MOUNT_POINT=

echo "Checking updater signature encoding"
SIGNATURE_BYTES="$WORK_ROOT/signature.bin"
base64 -D <"$SIGNATURE" >"$SIGNATURE_BYTES"
[[ -s "$SIGNATURE_BYTES" ]] || { echo "Updater signature decoded to an empty file." >&2; exit 1; }

echo "Extracting and verifying updater archive: $ARCHIVE"
ARCHIVE_ROOT="$WORK_ROOT/archive"
mkdir -p "$ARCHIVE_ROOT"
if tar -tzf "$ARCHIVE" | grep -Eq '(^/|(^|/)\.\.(\/|$))'; then
  echo "Updater archive contains an unsafe path." >&2
  exit 1
fi
tar -xzf "$ARCHIVE" -C "$ARCHIVE_ROOT"
ARCHIVE_APP=$(find_single_app "$ARCHIVE_ROOT")
verify_signed_app "$ARCHIVE_APP"

echo "Release artifacts passed for $TARGET ($EXPECTED_ARCH)."
