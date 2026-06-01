#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

APP_PATH="${APP_PATH:-$PWD/target/release/bundle/macos/Poha.app}"
RECORDINGS_DIR="${POHA_RECORDINGS_DIR:-$HOME/Library/Application Support/Poha/recordings}"
WAIT_SECONDS="${WAIT_SECONDS:-300}"
SKIP_BUILD=0
GUIDED=0

usage() {
  cat >&2 <<'EOF'
usage: script/run_live_test.sh [--no-build] [--guided]

Builds the packaged macOS app, verifies signing/entitlements, launches Poha,
triggers the tray audio diagnostic item, waits for live-test-report.json, and
prints the diagnostic result. Requires an unlocked desktop session.
EOF
}

trigger_audio_test() {
  local item_label="Run Quick Live Test"
  if [[ "$GUIDED" -eq 1 ]]; then
    item_label="Run Guided Audio Check"
  fi

  osascript <<APPLESCRIPT >/dev/null
tell application "System Events"
  tell process "Poha"
    click menu bar item 1 of menu bar 2
    delay 0.2
    set diagnosticsMenu to menu 1 of menu item "Troubleshooting" of menu 1 of menu bar item 1 of menu bar 2
    click menu item "$item_label" of diagnosticsMenu
  end tell
end tell
APPLESCRIPT
}

find_latest_report_since() {
  local start_epoch="$1"
  find "$RECORDINGS_DIR" -maxdepth 5 -name 'live-test-report.json' -print 2>/dev/null |
    while IFS= read -r f; do
      stat -f '%m %N' "$f"
    done |
    awk -v start="$start_epoch" '$1 >= start { sub(/^[0-9]+ /, ""); print }' |
    tail -1
}

wait_for_report_since() {
  local start_epoch="$1"
  local deadline=$((start_epoch + WAIT_SECONDS))
  local latest=""
  while [[ "$(date +%s)" -lt "$deadline" ]]; do
    latest=$(find_latest_report_since "$start_epoch")
    if [[ -n "$latest" ]]; then
      printf '%s\n' "$latest"
      return 0
    fi
    sleep 3
  done
  return 1
}

print_report() {
  local latest="$1"
  local status summary markdown
  status=$(jq -r '.status' "$latest")
  summary=$(jq -r '.summary' "$latest")
  markdown=$(jq -r '.paths.reportMarkdownPath // empty' "$latest")

  echo "report: $latest"
  echo "status: $status"
  echo "summary: $summary"
  if [[ -n "$markdown" ]]; then
    echo "markdown: $markdown"
  fi

  [[ "$status" == "passed" ]]
}

if_screen_locked() {
  [[ "$(ioreg -n Root -d1 2>/dev/null)" == *'"CGSSessionScreenIsLocked"=Yes'* ]]
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build)
      SKIP_BUILD=1
      shift
      ;;
    --guided)
      GUIDED=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  pnpm poha:build
fi

if [[ ! -d "$APP_PATH" ]]; then
  echo "Poha.app not found: $APP_PATH" >&2
  exit 2
fi

bundle_id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP_PATH/Contents/Info.plist")
if [[ "$bundle_id" != "com.iamadig.poha" ]]; then
  echo "Unexpected bundle id: $bundle_id" >&2
  exit 2
fi

if ! codesign -d --entitlements :- "$APP_PATH" 2>/dev/null | grep -q 'com.apple.security.device.audio-input'; then
  echo "Missing microphone entitlement on $APP_PATH" >&2
  exit 2
fi

signature_summary=$(codesign -dv "$APP_PATH" 2>&1 | sed -n '1,12p')
if grep -q 'Signature=adhoc' <<<"$signature_summary"; then
  echo "warning: app is ad-hoc signed; macOS may require a fresh privacy grant after rebuild" >&2
fi

if if_screen_locked; then
  echo "Screen is locked; unlock the Mac before running the UI-driven live test." >&2
  exit 5
fi

pkill -x poha 2>/dev/null || true
open -n "$APP_PATH"
sleep 2

tray_info=$(osascript <<'APPLESCRIPT'
tell application "System Events"
  tell process "Poha"
    click menu bar item 1 of menu bar 2
    delay 0.2
    set statusLabel to name of menu item 1 of menu 1 of menu bar item 1 of menu bar 2
    set permissionMenu to menu 1 of menu item "Permissions" of menu 1 of menu bar item 1 of menu bar 2
    set permissionLabels to {}
    repeat with mi in menu items of permissionMenu
      set end of permissionLabels to name of mi
    end repeat
    key code 53
    return statusLabel & linefeed & (permissionLabels as text)
  end tell
end tell
APPLESCRIPT
)
tray_status=$(printf '%s\n' "$tray_info" | sed -n '1p')
permission_labels=$(printf '%s\n' "$tray_info" | sed -n '2,$p')

echo "$tray_status"
echo "$permission_labels"

start_epoch=$(date +%s)

if [[ "$permission_labels" != *"✓ Microphone"* || "$permission_labels" != *"✓ System Audio"* ]]; then
  echo "Packaged app permissions are missing; asking Poha to write a preflight live-test report." >&2
  trigger_audio_test
  latest=$(wait_for_report_since "$start_epoch" || true)
  if [[ -z "$latest" ]]; then
    echo "No permission preflight live-test-report.json produced within ${WAIT_SECONDS}s" >&2
    echo "Open Poha tray > Permissions and re-grant Microphone/System Audio, then rerun this script." >&2
    exit 3
  fi
  print_report "$latest"
  exit $?
fi

trigger_audio_test
latest=$(wait_for_report_since "$start_epoch" || true)

if [[ -z "$latest" ]]; then
  echo "No live-test-report.json produced within ${WAIT_SECONDS}s" >&2
  exit 4
fi

print_report "$latest"
