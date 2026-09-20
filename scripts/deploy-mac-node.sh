#!/usr/bin/env bash
# Build the gateway on a macOS node, sign it with the Apple Development identity that lives in
# the Mac Studio keychain (stable identifier + team ⇒ macOS remembers the Local Network /
# TCC grants across rebuilds instead of prompting after every deploy), install, restart.
#
# usage: scripts/deploy-mac-node.sh <host> <advertise> <peers-csv> [release|dev]
#   host       ssh target of the node (e.g. 192.168.2.40); "local" = this machine
#   advertise  address the node announces to the cluster (e.g. 192.168.2.40:9400)
#   peers      comma-separated peer addresses
#   profile    release (default) or dev
set -euo pipefail
HOST=${1:?host}; ADV=${2:?advertise}; PEERS=${3:?peers}; PROFILE=${4:-release}
IDENTITY=${PROD_CODE_SIGN_IDENTITY:-"Apple Development: afonya@student.su (53BSRDD35M)"}
BUNDLE_ID=com.prod-code.gateway
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

REMOTE_BUILD=$(cat <<'RB'
set -e
export PATH=$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH
cd ~/prod-code 2>/dev/null || cd ~/Documents/workspace/prod-code
if [ "$PROFILE" = release ]; then
  git pull -q origin main; git log --oneline -1
  cargo build --release -p prod-code-gateway 2>&1 | tail -1
  echo "$PWD/target/release/prod-code-server"
else
  cargo build -p prod-code-gateway 2>&1 | tail -1
  echo "$PWD/target/debug/prod-code-server"
fi
RB
)

if [ "$HOST" = local ]; then
  BIN=$(PROFILE=$PROFILE bash -c "$REMOTE_BUILD" | tee /dev/stderr | tail -1)
  cp "$BIN" "$TMP/prod-code-server"
else
  BIN=$(ssh "$HOST" "PROFILE=$PROFILE bash -s" <<<"$REMOTE_BUILD" | tee /dev/stderr | tail -1)
  scp -q "$HOST:$BIN" "$TMP/prod-code-server"
fi

codesign -f -s "$IDENTITY" -i "$BUNDLE_ID" "$TMP/prod-code-server"
codesign -dv --verbose=2 "$TMP/prod-code-server" 2>&1 | grep -E "^Identifier|TeamIdentifier"

PLIST=$(cat <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.prod-code.gateway</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/alex09x/.local/bin/prod-code-server</string>
        <string>--bind</string><string>0.0.0.0:9400</string>
        <string>--storage</string><string>/Users/alex09x/prod-code-storage/workspaces</string>
        <string>--advertise</string><string>$ADV</string>
        <string>--peers</string><string>$PEERS</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>ProcessType</key><string>Interactive</string>
    <key>StandardOutPath</key><string>/Users/alex09x/Library/Logs/prod-code-gateway.log</string>
    <key>StandardErrorPath</key><string>/Users/alex09x/Library/Logs/prod-code-gateway.log</string>
</dict>
</plist>
PL
)

printf '%s\n' "$PLIST" > "$TMP/com.prod-code.gateway.plist"

INSTALL=$(cat <<'IN'
set -e
mkdir -p ~/.local/bin ~/Library/LaunchAgents
mv -f ~/.local/bin/prod-code-server.new ~/.local/bin/prod-code-server
codesign -v ~/.local/bin/prod-code-server
mv -f ~/Library/LaunchAgents/com.prod-code.gateway.plist.new ~/Library/LaunchAgents/com.prod-code.gateway.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.prod-code.gateway.plist 2>/dev/null || true
launchctl kickstart -k gui/$(id -u)/com.prod-code.gateway
sleep 2
launchctl print gui/$(id -u)/com.prod-code.gateway | grep -E "state = " | head -1
IN
)

# The nodes' login shell is fish, so everything runs through `bash -s` with files staged first.
if [ "$HOST" = local ]; then
  cp "$TMP/prod-code-server" ~/.local/bin/prod-code-server.new
  cp "$TMP/com.prod-code.gateway.plist" ~/Library/LaunchAgents/com.prod-code.gateway.plist.new
  bash -c "$INSTALL"
else
  scp -q "$TMP/prod-code-server" "$HOST:.local/bin/prod-code-server.new"
  scp -q "$TMP/com.prod-code.gateway.plist" "$HOST:Library/LaunchAgents/com.prod-code.gateway.plist.new"
  ssh "$HOST" bash -s <<<"$INSTALL"
fi
