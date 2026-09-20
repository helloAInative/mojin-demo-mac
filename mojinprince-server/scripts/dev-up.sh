#!/bin/zsh
# scripts/dev-up.sh — 后台构建并启动 mojinprince-server，写日志到 logs/server.log。
#
# 用法：
#   ./scripts/dev-up.sh                  # 默认 127.0.0.1:8732，本项目 data/dev.db
#   BIND_ADDR=0.0.0.0:8732 ./scripts/dev-up.sh
#   STOP=1 ./scripts/dev-up.sh          # 停掉后台实例
#
# 配套：
#   ./scripts/smoke.sh   - 启动后跑冒烟
#   ./scripts/load-test.sh - 启动后跑延迟基准

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p logs data

PID_FILE="$ROOT/data/server.pid"
LOG_FILE="$ROOT/logs/server.log"

stop() {
  if [[ -f "$PID_FILE" ]]; then
    local pid
    pid="$(cat "$PID_FILE")"
    if kill -0 "$pid" 2>/dev/null; then
      echo "stopping pid=$pid"
      kill "$pid" || true
      sleep 1
      kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$PID_FILE"
  fi
  exit 0
}

if [[ "${STOP:-0}" == "1" ]]; then
  stop
fi

# 已运行则提示
if [[ -f "$PID_FILE" ]] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
  echo "already running (pid=$(cat "$PID_FILE"))，stop with: $0 STOP=1"
  exit 0
fi

export BIND_ADDR="${BIND_ADDR:-127.0.0.1:8732}"
export DATABASE_URL="${DATABASE_URL:-sqlite://$ROOT/data/dev.db}"
export RUST_LOG="${RUST_LOG:-info,actix_web=info,sqlx=warn}"
# 开发模式：编 debug 就行，省时间
cargo build --offline --quiet

: > "$LOG_FILE"
nohup "$ROOT/target/debug/mojinprince-server" >>"$LOG_FILE" 2>&1 &
echo $! > "$PID_FILE"

# 等就绪：每 200ms ping 一次 /health，最多 10s
BASE_URL="http://${BIND_ADDR}"
ok=0
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
  if curl -fsS "$BASE_URL/health" >/dev/null 2>&1; then
    ok=1
    break
  fi
  sleep 0.5
done

if [[ $ok -ne 1 ]]; then
  echo "server failed to come up; tail of log:"
  tail -n 40 "$LOG_FILE"
  exit 1
fi

echo "up: $BASE_URL  pid=$(cat "$PID_FILE")  log=$LOG_FILE"
echo "stop:  $0 STOP=1   or   kill $(cat "$PID_FILE")"