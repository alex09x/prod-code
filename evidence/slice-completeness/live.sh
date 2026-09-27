#!/usr/bin/env bash
# Starts a standalone gateway from this revision and runs the live code_slice test against it.
set -euo pipefail
cargo build -p prod-code-gateway --bin prod-code-server
cargo test -p prod-code-mcp --test slice_completeness --no-run
STORAGE=$(mktemp -d); TESTHOME=$(mktemp -d); PORT=19531
PROD_CODE_STORAGE="$STORAGE" PROD_CODE_PEERS= PROD_CODE_BIND=127.0.0.1:$PORT RUST_LOG=info \
  ./target/debug/prod-code-server > "$STORAGE/gateway.log" 2>&1 &
GW=$!
trap 'kill $GW 2>/dev/null || true; wait $GW 2>/dev/null || true; rm -rf "$STORAGE" "$TESTHOME"' EXIT
for _ in $(seq 1 60); do
  if (exec 3<>/dev/tcp/127.0.0.1/$PORT) 2>/dev/null; then break; fi
  sleep 1
done
(exec 3<>/dev/tcp/127.0.0.1/$PORT) || { echo "gateway did not listen"; tail -20 "$STORAGE/gateway.log"; exit 1; }
echo "gateway listening on 127.0.0.1:$PORT (pid $GW)"
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" HOME="$TESTHOME" \
  PROD_CODE_LIVE_GATEWAY=127.0.0.1:$PORT \
  cargo test -p prod-code-mcp --test slice_completeness -- --ignored --nocapture
