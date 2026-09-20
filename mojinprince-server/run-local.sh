#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
export BIND_ADDR="${BIND_ADDR:-127.0.0.1:8732}"
export DATABASE_URL="${DATABASE_URL:-sqlite://$ROOT/data/mojinprince.db}"

mkdir -p "$ROOT/data"
cargo build --offline --manifest-path "$ROOT/Cargo.toml"
exec "$ROOT/target/debug/mojinprince-server"
