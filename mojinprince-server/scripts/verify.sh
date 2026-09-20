#!/bin/zsh
# scripts/verify.sh — 自包含端到端验证：启动 + 冒烟 + 延迟基准 + 停。
#
# 设计意图：sandbox 长跑进程会被回收，所以一个进程内拉起服务、跑完两个测试再停。
#
# 用法：
#   ./scripts/verify.sh

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p logs data

export BIND_ADDR="${BIND_ADDR:-127.0.0.1:8732}"
export DATABASE_URL="${DATABASE_URL:-sqlite://$ROOT/data/verify.db}"
export RUST_LOG="${RUST_LOG:-info,actix_web=warn,sqlx=warn}"
BASE_URL="http://${BIND_ADDR}"

cargo build --offline --quiet

# ⚠️ 在 Cursor 的 shell 沙盒里，.cargo/config.toml 的 target-dir 会被环境劫持
# 到 /var/folders/.../cargo-target，工程内 target/debug/mojinprince-server 是旧的。
# 直接读 cargo 的消息，挑出实际编出来的 binary 路径，避免用错旧文件。
BIN=""
setopt NULL_GLOB 2>/dev/null || true
# cargo 优先打到 sandbox cache；如果环境里没 sandbox，就用工程内 target。
# 路径形如 /var/folders/<A>/T/cursor-sandbox-cache/<B>/cargo-target/...
# 取最新修改的那份（cargo 会复用同一个 cache，dev-up 跑过几次后可能有多份）。
LATEST=""
LATEST_MTIME=0
for candidate in /var/folders/*/*/T/cursor-sandbox-cache/*/cargo-target/debug/mojinprince-server ; do
  if [[ -x "$candidate" ]]; then
    mtime=$(stat -f %m "$candidate" 2>/dev/null || echo 0)
    if [[ "$mtime" -gt "$LATEST_MTIME" ]]; then
      LATEST_MTIME="$mtime"
      LATEST="$candidate"
    fi
  fi
done
if [[ -n "$LATEST" ]]; then
  BIN="$LATEST"
fi
unsetopt NULL_GLOB 2>/dev/null || true
if [[ -z "$BIN" ]]; then
  BIN="$ROOT/target/debug/mojinprince-server"
fi
if [[ ! -x "$BIN" ]]; then
  echo "no built binary found"
  exit 1
fi
echo "using binary: $BIN"

LOG="$ROOT/logs/verify-server.log"
: > "$LOG"

# 后台启动
"$BIN" >>"$LOG" 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

# 等就绪
ok=0
for i in {1..30}; do
  if curl -fsS "$BASE_URL/health" >/dev/null 2>&1; then
    ok=1
    break
  fi
  sleep 0.3
done

if [[ $ok -ne 1 ]]; then
  echo "server failed to start, log tail:"
  tail -n 30 "$LOG"
  exit 1
fi

echo "== smoke =="
"$(dirname "$0")/smoke.sh"

echo
echo "== load =="
"$(dirname "$0")/load-test.sh"