#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LABEL="com.mojinprince.server"
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
INSTALL_DIR="$HOME/Library/Application Support/MojinPrinceServer"
DATA_DIR="$INSTALL_DIR/data"
LOG_DIR="$INSTALL_DIR/logs"
BINARY="$INSTALL_DIR/mojinprince-server"
DATABASE_URL="sqlite://$DATA_DIR/mojinprince.db"

mkdir -p "$DATA_DIR" "$LOG_DIR" "$HOME/Library/LaunchAgents"
cargo build --release --offline --manifest-path "$ROOT/Cargo.toml"
cp "$ROOT/target/release/mojinprince-server" "$BINARY"
chmod 755 "$BINARY"

sed \
  -e "s|__SERVER_BINARY__|$BINARY|g" \
  -e "s|__DATABASE_URL__|$DATABASE_URL|g" \
  -e "s|__STDOUT_LOG__|$LOG_DIR/server.log|g" \
  -e "s|__STDERR_LOG__|$LOG_DIR/server.err|g" \
  "$ROOT/com.mojinprince.server.plist.template" > "$PLIST"

plutil -lint "$PLIST" >/dev/null
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart -k "$DOMAIN/$LABEL"

echo "✅ 行情网关已安装并启动：http://127.0.0.1:8732"
echo "   配置：$PLIST"
echo "   程序：$BINARY"
echo "   日志：$LOG_DIR/server.log / server.err"
