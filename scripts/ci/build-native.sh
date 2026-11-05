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
export MITTS_JOBS=2
if test "$(uname -s)" = Darwin; then export MITTS_JOBS=1; fi
mitts build --native --release --locked -p prod-code-client --bins
mitts build --native --release --locked -p prod-code-gateway --bins
out="$RUNNER_TEMP/native-assets"
mkdir -p "$out"
for binary in prod-code prod-code-server; do
    if ! version=$("target/release/$binary" --version); then
        printf 'Version probe failed for %s\n' "$binary" >&2
        exit 1
    fi
    # Linux startup diagnostics can precede the final canonical version banner.
    version=${version##*$'\n'}
    if test "$version" != "$binary $RELEASE_VERSION"; then
        printf 'Unexpected version for %s: expected "%s", got "%s"\n' \
            "$binary" "$binary $RELEASE_VERSION" "$version" >&2
        exit 1
    fi
    install -m 755 "target/release/$binary" "$out/$binary-$RELEASE_TARGET"
done
if test "$(uname -s)" = Linux; then
    python3 ../automation/scripts/packaging/glibc_compat.py --baseline 2.35 \
        "$out/prod-code-$RELEASE_TARGET" "$out/prod-code-server-$RELEASE_TARGET"
fi
file "$out"/*
COPYFILE_DISABLE=1 tar -cf "$RUNNER_TEMP/native-assets.tar" -C "$out" .
