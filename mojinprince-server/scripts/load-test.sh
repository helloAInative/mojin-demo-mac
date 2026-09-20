#!/bin/zsh
# scripts/load-test.sh — 简单延迟基准（不依赖 gawk）。
#
# 用法：
#   ./scripts/dev-up.sh && ./scripts/load-test.sh
#
# 默认：
#   CODE=sh600460
#   N=200 并发 8 个 worker
#
# 输出：success / P50 / P95 / max（毫秒）

set -euo pipefail
BASE_URL="${BASE_URL:-http://127.0.0.1:8732}"
CODE="${CODE:-sh600460}"
N="${N:-200}"
WORKERS="${WORKERS:-8}"

URL="$BASE_URL/api/v1/quote/$CODE"

echo "load test: $URL  N=$N  workers=$WORKERS"

OUT="$(mktemp -t load.XXXXXX)"
trap 'rm -f "$OUT" /tmp/_lat.txt' EXIT

seq "$N" | xargs -n1 -P"$WORKERS" -I{} \
  curl -s -o /dev/null -w '%{time_total}\n%{http_code}\n' "$URL" \
  >>"$OUT"

# 偶数行是状态码、奇数行是时间 → 拆两个文件
# 把时间（秒）转毫秒并排序，方便按行号取分位数
awk 'NR%2==1 {print $1*1000}' "$OUT" | sort -n > /tmp/_lat.txt
OK=$(awk 'NR%2==0 && $1==200' "$OUT" | wc -l | tr -d ' ')
TOTAL=$(awk 'NR%2==0' "$OUT" | wc -l | tr -d ' ')
BAD=$(awk 'NR%2==0 && $1!=200' "$OUT" | wc -l | tr -d ' ')

# 分位数（已排序的毫秒数组）
P50=$(sed -n "$(awk "BEGIN{print int($TOTAL*0.50)+1}")p" /tmp/_lat.txt)
P95=$(sed -n "$(awk "BEGIN{print int($TOTAL*0.95)+1}")p" /tmp/_lat.txt)
P99=$(sed -n "$(awk "BEGIN{print int($TOTAL*0.99)+1}")p" /tmp/_lat.txt)
MAX=$(tail -n1 /tmp/_lat.txt)

printf 'samples: %s\n' "$TOTAL"
printf 'success: %s\n' "$OK"
printf 'failure: %s\n' "$BAD"
printf 'P50:     %sms\n' "$P50"
printf 'P95:     %sms\n' "$P95"
printf 'P99:     %sms\n' "$P99"
printf 'max:     %sms\n' "$MAX"