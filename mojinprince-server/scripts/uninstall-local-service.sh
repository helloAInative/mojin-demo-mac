#!/bin/zsh
set -euo pipefail

LABEL="com.mojinprince.server"
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
rm -f "$PLIST"
echo "✅ 已停用本机行情网关；Application Support 中的数据库与日志已保留。"
