#!/bin/bash
# screenshotr installer. Run from the extracted package directory.
set -euo pipefail

APP_NAME="ScreenshotR"
BIN="screenshotr"
BUNDLE_ID="com.keithsimon.screenshotr"
PORT="${SCREENSHOTR_PORT:-8765}"
BIND="${SCREENSHOTR_BIND:-0.0.0.0:$PORT}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_APP="$HERE/$APP_NAME.app"
INSTALL_DIR="$HOME/Applications"
APP_PATH="$INSTALL_DIR/$APP_NAME.app"
PLIST="$HOME/Library/LaunchAgents/$BUNDLE_ID.plist"
TOKEN_FILE="$HOME/.config/screenshotr/token"
LOG_DIR="$HOME/Library/Logs"

say()  { printf '  %s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
die()  { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

step "Checking this machine"

[ -d "$SRC_APP" ] || die "$APP_NAME.app not found next to this script."

arch="$(uname -m)"
[ "$arch" = "arm64" ] || die "This package is arm64 (Apple Silicon) only; this Mac is $arch."
say "architecture: arm64"

macos="$(sw_vers -productVersion)"
major="${macos%%.*}"
[ "$major" -ge 14 ] || die "macOS 14.0 or later required; this Mac runs $macos."
say "macOS: $macos"

step "Verifying code signature"

# The Screen Recording grant is bound to the signature's designated requirement,
# so a broken or stripped signature means the grant can never stick.
codesign --verify --strict "$SRC_APP" 2>/dev/null \
  || die "Signature is invalid. The package was likely corrupted in transfer."
say "signature valid"
say "$(codesign -d -r- "$SRC_APP" 2>&1 | grep '^designated' | cut -c1-72)..."

# Gatekeeper only assesses files carrying a quarantine flag. This app is signed
# but not notarized (that needs a Developer ID cert), so if the package arrived
# via a browser or AirDrop macOS will refuse to launch it. Transfers over
# scp/rsync/USB carry no quarantine flag and skip this entirely.
if xattr -p com.apple.quarantine "$SRC_APP" >/dev/null 2>&1; then
    step "Clearing quarantine flag"
    say "This package is quarantined, which means it arrived via a browser,"
    say "AirDrop, or similar. It is signed but not notarized, so macOS would"
    say "block it. Removing the flag since you built and transferred this app."
    xattr -dr com.apple.quarantine "$SRC_APP"
    say "cleared"
fi

step "Installing to $APP_PATH"

if launchctl print "gui/$(id -u)/$BUNDLE_ID" >/dev/null 2>&1; then
    say "stopping running agent"
    launchctl bootout "gui/$(id -u)/$BUNDLE_ID" 2>/dev/null || true
fi

mkdir -p "$INSTALL_DIR" "$LOG_DIR"
rm -rf "$APP_PATH"
cp -R "$SRC_APP" "$APP_PATH"
say "installed"

step "Setting up API token"

mkdir -p "$(dirname "$TOKEN_FILE")"
if [ -s "$TOKEN_FILE" ]; then
    say "keeping existing token at $TOKEN_FILE"
else
    LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 48 > "$TOKEN_FILE"
    say "generated new token"
fi
chmod 600 "$TOKEN_FILE"

step "Installing LaunchAgent"

cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$BUNDLE_ID</string>
    <key>ProgramArguments</key>
    <array>
        <string>$APP_PATH/Contents/MacOS/$BIN</string>
    </array>
    <key>AssociatedBundleIdentifiers</key>
    <string>$BUNDLE_ID</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SCREENSHOTR_TOKEN_FILE</key>
        <string>$TOKEN_FILE</string>
        <key>SCREENSHOTR_BIND</key>
        <string>$BIND</string>
        <key>RUST_LOG</key>
        <string>info</string>
    </dict>
    <key>StandardOutPath</key>
    <string>$LOG_DIR/screenshotr.log</string>
    <key>StandardErrorPath</key>
    <string>$LOG_DIR/screenshotr.err.log</string>
</dict>
</plist>
PLIST_EOF

plutil -lint "$PLIST" >/dev/null || die "generated LaunchAgent plist is invalid"
launchctl bootstrap "gui/$(id -u)" "$PLIST"
launchctl kickstart -k "gui/$(id -u)/$BUNDLE_ID"
say "agent loaded"

step "Waiting for service"

# First run has no Screen Recording grant: the app opens System Settings and
# exits, and launchd restarts it every 10s until the grant lands.
ok=false
for _ in $(seq 1 10); do
    sleep 1
    if curl -fsS --max-time 2 "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
        ok=true
        break
    fi
done

echo
if $ok; then
    health="$(curl -fsS "http://127.0.0.1:$PORT/healthz")"
    echo "screenshotr is running."
    echo "  $health"
    echo
    echo "Try it:"
    echo "  curl -H \"Authorization: Bearer \$(cat $TOKEN_FILE)\" \\"
    echo "       http://localhost:$PORT/screenshot -o shot.jpg"
else
    echo "Installed, but not serving yet — it needs Screen Recording permission."
    echo
    echo "  1. System Settings > Privacy & Security > Screen Recording"
    echo "     (it should have opened automatically)"
    echo "  2. Enable $APP_NAME"
    echo "  3. It starts serving within ~10s. Verify with:"
    echo "       curl -s http://localhost:$PORT/healthz"
    echo
    echo "  Logs: $LOG_DIR/screenshotr.err.log"
fi

echo
echo "Token:     $TOKEN_FILE"
echo "Listening: $BIND"
if [[ "$BIND" == 0.0.0.0:* ]]; then
    echo "           ^ reachable from your LAN over plain HTTP, bearer-token gated."
    echo "             Re-run with SCREENSHOTR_BIND=127.0.0.1:$PORT to restrict to this Mac."
fi
echo "Uninstall: ./uninstall.sh"
