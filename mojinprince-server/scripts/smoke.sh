#!/bin/zsh
# scripts/smoke.sh — 冒烟测试。假定 server 已经在监听 BIND_ADDR。
#
# 校验：
#   /health                            → 200
#   /api/v1/quote/sh600460             → 200，price > 0
#   /api/v1/quote/sz000001             → 200，price > 0
#   /api/v1/quote/bj835899             → 200（或 502，周末不开盘）
#   /api/v1/quote/sh600460/minutes?limit=10  → 200，len >= 1
#   /api/v1/quote/sh600460/days?limit=10     → 200，len >= 1
#   /api/v1/quote/notacode             → 400
#   /swagger-ui/                        → 200
#   /api-docs/openapi.json              → 200，paths 含 /health
#
# 用法：
#   ./scripts/dev-up.sh && ./scripts/smoke.sh

set -euo pipefail
BASE_URL="${BASE_URL:-http://127.0.0.1:8732}"

pass=0
fail=0
report() {
  local label="$1" want="$2" got="$3"
  if [[ "$want" == "$got" ]]; then
    printf '  ✅ %-50s  %s\n' "$label" "$got"
    pass=$((pass + 1))
  else
    printf '  ❌ %-50s  want=%s got=%s\n' "$label" "$want" "$got"
    fail=$((fail + 1))
  fi
}

http_code() { curl -s -o /dev/null -w '%{http_code}' "$1"; }

# 1. /health
report "/health" 200 "$(http_code "$BASE_URL/health")"

# 2. 实时报价
for body in "$BASE_URL/api/v1/quote/sh600460" \
           "$BASE_URL/api/v1/quote/sz000001"; do
  resp=$(curl -fsS "$body")
  code=$(printf '%s' "$resp" | python3 -c 'import json,sys;print(json.load(sys.stdin)["price"])')
  hc=$(http_code "$body")
  report "$(basename "$body") http" 200 "$hc"
  report "$(basename "$body") price >0" "yes" "$([[ "$code" != "0.0" && "$code" != "0" ]] && echo yes || echo no)"
done

# 3. 北交所：周末/节假日可能没数据，允许 502
bj_status=$(http_code "$BASE_URL/api/v1/quote/bj835899")
if [[ "$bj_status" == "200" || "$bj_status" == "502" ]]; then
  report "/api/v1/quote/bj835899 (allow 200|502)" "$bj_status" "$bj_status"
else
  report "/api/v1/quote/bj835899 (allow 200|502)" "ok" "unexpected=$bj_status"
fi

# 4. 分时（容忍 502：休市时腾讯分时接口可能返回空 → 我们 upstream_exhausted）
mins_resp=$(curl -s -w '\n%{http_code}' "$BASE_URL/api/v1/quote/sh600460/minutes?limit=10")
mins_code=$(printf '%s' "$mins_resp" | tail -n1)
if [[ "$mins_code" == "200" ]]; then
  mins=$(printf '%s' "$mins_resp" | sed '$d' | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')
  report "minutes sh600460 len>=1" "yes" "$([[ "$mins" -ge 1 ]] && echo yes || echo no)"
else
  report "minutes sh600460 (allow 200|502)" "ok" "$mins_code"
fi

# 5. 日 K（同上）
days_resp=$(curl -s -w '\n%{http_code}' "$BASE_URL/api/v1/quote/sh600460/days?limit=10")
days_code=$(printf '%s' "$days_resp" | tail -n1)
if [[ "$days_code" == "200" ]]; then
  days=$(printf '%s' "$days_resp" | sed '$d' | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')
  report "days sh600460 len>=1" "yes" "$([[ "$days" -ge 1 ]] && echo yes || echo no)"
else
  report "days sh600460 (allow 200|502)" "ok" "$days_code"
fi

# 6. 错误码
report "bad code returns 400" 400 "$(http_code "$BASE_URL/api/v1/quote/notacode")"

# 7. Swagger UI
report "/swagger-ui/" 200 "$(http_code "$BASE_URL/swagger-ui/")"

# 8. OpenAPI JSON
report "/api-docs/openapi.json" 200 "$(http_code "$BASE_URL/api-docs/openapi.json")"
paths=$(curl -fsS "$BASE_URL/api-docs/openapi.json" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(",".join(sorted(d["paths"].keys())))')
report "openapi paths contains /health" "ok" "$([[ "$paths" == *"/health"* ]] && echo ok || echo "missing ($paths)")"

echo
echo "summary: $pass passed, $fail failed"
exit $(( fail > 0 ? 1 : 0 ))