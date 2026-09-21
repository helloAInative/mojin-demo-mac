#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
SRC="$ROOT/Sources"
OUT="$ROOT/build"
APP_NAME="摸金小王子"
APP="$OUT/${APP_NAME}.app"
BIN_NAME="JJWDWidget"
SDK="${SDK_PATH:-$(xcrun --show-sdk-path)}"
TARGET="arm64-apple-macosx14.0"

mkdir -p "$OUT"
rm -rf "$APP"

echo "→ Preparing icon…"
if [[ -s "$ROOT/Resources/AppIcon.icns" ]]; then
  cp "$ROOT/Resources/AppIcon.icns" "$OUT/AppIcon.icns"
else
  swift "$ROOT/Tools/generate-icon.swift" "$OUT" >/dev/null
fi

echo "→ Compiling…"
swiftc -parse-as-library \
  "$SRC"/Models.swift \
  "$SRC"/MACD.swift \
  "$SRC"/AIService.swift \
  "$SRC"/AIDegradeGovernor.swift \
  "$SRC"/AppSettings.swift \
  "$SRC"/MarketService.swift \
  "$SRC"/GatewayMarketClient.swift \
  "$SRC"/AlertService.swift \
  "$SRC"/SignalTimeline.swift \
  "$SRC"/AIUsageLedger.swift \
  "$SRC"/LLMProvider.swift \
  "$SRC"/OrderTicket.swift \
  "$SRC"/ReviewDesk.swift \
  "$SRC"/StrategyEngine.swift \
  "$SRC"/StopTakeAdvisor.swift \
  "$SRC"/PortfolioBuilder.swift \
  "$SRC"/MarketStore.swift \
  "$SRC"/Charts.swift \
  "$SRC"/ContentView.swift \
  "$SRC"/JJWDWidgetApp.swift \
  -o "$OUT/$BIN_NAME" \
  -sdk "$SDK" \
  -target "$TARGET" \
  -framework SwiftUI \
  -framework AppKit \
  -framework Foundation \
  -framework UserNotifications \
  -framework ServiceManagement \
  -framework UniformTypeIdentifiers \
  -framework Security \
  -O

echo "→ Packaging ${APP_NAME}.app…"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$OUT/$BIN_NAME" "$APP/Contents/MacOS/$BIN_NAME"
cp "$ROOT/Info.plist" "$APP/Contents/Info.plist"
cp "$OUT/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"
codesign --force --deep --sign - "$APP" >/dev/null
xattr -cr "$APP" 2>/dev/null || true

if [[ "${BUILD_ONLY:-0}" == "1" ]]; then
  echo "✅ Built: $APP"
  exit 0
fi

# Desktop
DEST_DESK="$HOME/Desktop/${APP_NAME}.app"
rm -rf "$DEST_DESK"
cp -R "$APP" "$DEST_DESK"
xattr -cr "$DEST_DESK" 2>/dev/null || true

# /Applications（开机自启更稳）
DEST_APP="/Applications/${APP_NAME}.app"
rm -rf "$DEST_APP"
cp -R "$APP" "$DEST_APP"
xattr -cr "$DEST_APP" 2>/dev/null || true

# 清掉旧名
rm -rf "$HOME/Desktop/捷捷微电摸鱼.app" "/Applications/捷捷微电摸鱼.app" 2>/dev/null || true

echo "✅ Built: $APP"
echo "✅ Desktop: $DEST_DESK"
echo "✅ Applications: $DEST_APP"
echo "   打开: open \"$DEST_APP\""
echo "   菜单栏右上角看报价；点击展开面板。"
