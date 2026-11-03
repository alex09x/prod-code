#!/usr/bin/env bash
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
set -euo pipefail

: "${RELEASE_VERSION:?}" "${RELEASE_TARGET:?}" "${RUNNER_TEMP:?}"
case "$RELEASE_TARGET" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|aarch64-apple-darwin) ;;
    *) echo 'Unsupported release target' >&2; exit 1 ;;
esac
actual=$(rustc -vV | awk '/^host:/ { print $2 }')
test "$actual" = "$RELEASE_TARGET"
export CARGO_INCREMENTAL=0
export CARGO_ENCODED_RUSTFLAGS=''
jobs=2
if test "$(uname -s)" = Darwin; then jobs=1; fi
cargo build --release --locked -p prod-code-client -p prod-code-gateway --bins -j "$jobs"
out="$RUNNER_TEMP/native-assets"
mkdir -p "$out"
for binary in prod-code prod-code-server; do
    version=$("target/release/$binary" --version)
    test "$version" = "$binary $RELEASE_VERSION"
    install -m 755 "target/release/$binary" "$out/$binary-$RELEASE_TARGET"
done
if test "$(uname -s)" = Linux; then
    python3 ../automation/scripts/packaging/glibc_compat.py --baseline 2.35 \
        "$out/prod-code-$RELEASE_TARGET" "$out/prod-code-server-$RELEASE_TARGET"
fi
file "$out"/*
COPYFILE_DISABLE=1 tar -cf "$RUNNER_TEMP/native-assets.tar" -C "$out" .
