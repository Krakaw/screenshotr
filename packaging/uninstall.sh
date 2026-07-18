#!/bin/bash
# Removes screenshotr. Keeps the token unless --purge is passed.
set -euo pipefail

APP_NAME="ScreenshotR"
BUNDLE_ID="com.keithsimon.screenshotr"
APP_PATH="$HOME/Applications/$APP_NAME.app"
PLIST="$HOME/Library/LaunchAgents/$BUNDLE_ID.plist"
CONFIG_DIR="$HOME/.config/screenshotr"

purge=false
[ "${1:-}" = "--purge" ] && purge=true

echo "==> Stopping agent"
launchctl bootout "gui/$(id -u)/$BUNDLE_ID" 2>/dev/null && echo "  stopped" || echo "  not running"

echo "==> Removing files"
rm -f "$PLIST"       && echo "  removed LaunchAgent"
rm -rf "$APP_PATH"   && echo "  removed $APP_PATH"

if $purge; then
    rm -rf "$CONFIG_DIR"
    echo "  removed $CONFIG_DIR (token)"
else
    echo "  kept token at $CONFIG_DIR/token (use --purge to remove)"
fi

echo
echo "Done. macOS keeps a Screen Recording entry for $APP_NAME in"
echo "System Settings > Privacy & Security; remove it there if you want it gone."
