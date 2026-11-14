#!/usr/bin/env bash
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
# Lists, for every rustup toolchain on this machine, the files no installed component owns.
#
# rustup records what each installed component put into a toolchain in
# lib/rustlib/manifest-<component>, and the installed components in lib/rustlib/components. A
# file in the toolchain that none of those manifests names is one rustup knows nothing about;
# when a component rustup installs later ships a file of that name, the install stops with
# "detected conflict" and the command that asked for the toolchain never starts (#2029).
# docs/build-node-toolchains.md has the repair.
#
# usage: scripts/rustup-unowned-files.sh [--all]
#   --all   list every unowned file (the default lists at most 20 per toolchain)
# Reads only. RUSTUP_HOME names the rustup directory (default ~/.rustup).
set -u

limit=20
[ "${1:-}" = "--all" ] && limit=0
home="${RUSTUP_HOME:-$HOME/.rustup}"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for toolchain in "$home"/toolchains/*/; do
  toolchain="${toolchain%/}"
  records="$toolchain/lib/rustlib"
  [ -f "$records/components" ] || continue
  : > "$work/owned"
  : > "$work/dirs"
  while IFS= read -r component; do
    [ -n "$component" ] || continue
    if [ -f "$records/manifest-$component" ]; then
      sed -n 's/^file://p' "$records/manifest-$component" >> "$work/owned"
      sed -n 's/^dir://p' "$records/manifest-$component" >> "$work/dirs"
    else
      echo "$(basename "$toolchain"): component $component has no manifest"
    fi
  done < "$records/components"
  LC_ALL=C sort -u -o "$work/owned" "$work/owned"
  (cd "$toolchain" && find . \( -type f -o -type l \) | sed 's#^\./##') \
    | grep -v -E '^lib/rustlib/(components|manifest-[^/]*|multirust-[^/]*|rust-installer-version)$' \
    | LC_ALL=C sort -u > "$work/present"
  LC_ALL=C comm -23 "$work/present" "$work/owned" \
    | awk -v dirs="$work/dirs" '
        BEGIN { while ((getline dir < dirs) > 0) owned[++count] = dir "/" }
        { for (i = 1; i <= count; i++) if (index($0, owned[i]) == 1) next; print }
      ' > "$work/unowned"
  found=$(wc -l < "$work/unowned" | tr -d ' ')
  echo "$(basename "$toolchain"): $found file(s) no installed component owns"
  if [ "$limit" -eq 0 ] || [ "$found" -le "$limit" ]; then
    sed 's/^/    /' "$work/unowned"
  else
    head -n "$limit" "$work/unowned" | sed 's/^/    /'
    echo "    ... and $((found - limit)) more (--all lists them)"
  fi
done
