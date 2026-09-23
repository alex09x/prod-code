#!/usr/bin/env python3
"""Per-file coverage, and a gate that fails when a file is under the bar.

A workspace total hides exactly what matters: one large untested file drags it down while a
dozen well covered ones hold it up, and the number moves for reasons nobody can point at. This
reports every file and fails on the files themselves.

Run it on a build node, never on the developer's machine:

    prod-code exec --timeout-secs 1800 --no-pull -- \\
        bash -lc 'python3 scripts/coverage.py --min 80'

    python3 scripts/coverage.py                  # report only, exit 0
    python3 scripts/coverage.py --min 80         # fail if any file is under 80% of regions
    python3 scripts/coverage.py --min 80 crates/prod-code-mcp/src/schema.rs   # only these
    python3 scripts/coverage.py --report target/coverage-report.json --min 80 FILE  # reuse the last run, no rebuild

Every run keeps its report at `target/coverage-report.json`. A file with no code (a crate root of
`pub mod` and `pub use`) is listed as `no code` rather than failing the gate.

`cargo llvm-cov` must be installed (`cargo install cargo-llvm-cov` plus the
`llvm-tools-preview` component).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys

# Files nothing is expected to cover: generated code, or a binary's entry point that only wires
# arguments into functions that are tested. Keep this list short and say why.
# Files nothing is expected to cover. Empty, and worth keeping that way: an exemption is a
# file nobody has to think about again.
EXEMPT: dict[str, str] = {}


def repo_root() -> str:
    """The workspace root: the checkout, or the copy of it a build node holds (no `.git` there,
    because only the files are synced)."""
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
    )
    if out.returncode == 0 and out.stdout.strip():
        return out.stdout.strip()
    return os.path.realpath(os.getcwd())


def collect(report_path: str | None, root: str) -> dict:
    if report_path:
        with open(report_path) as handle:
            return json.load(handle)
    # Kept, not temporary: an instrumented build takes minutes, and a rerun over other files can
    # read this with `--report` instead of building again.
    out_path = os.path.join(root, "target", "coverage-report.json")
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    subprocess.run(
        [
            "cargo",
            "llvm-cov",
            "--workspace",
            "--json",
            "--summary-only",
            "--output-path",
            out_path,
        ],
        check=True,
    )
    print(f"report kept at {out_path} (reuse it with --report)", file=sys.stderr)
    with open(out_path) as handle:
        return json.load(handle)


def files_of(report: dict, root: str) -> list[tuple[str, float, int, int]]:
    """(path relative to the repository, region coverage, covered regions, total regions)."""
    rows = []
    for data in report.get("data", []):
        for entry in data.get("files", []):
            path = entry.get("filename", "")
            if not path.startswith(root):
                continue
            relative = os.path.relpath(path, root)
            if "/tests/" in relative or relative.startswith("tests/"):
                continue
            regions = entry.get("summary", {}).get("regions", {})
            total = regions.get("count", 0)
            covered = regions.get("covered", 0)
            if total == 0:
                continue
            rows.append((relative, 100.0 * covered / total, covered, total))
    rows.sort(key=lambda row: row[1])
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--min",
        type=float,
        default=None,
        help="fail when a file's region coverage is under this percentage",
    )
    parser.add_argument(
        "--report",
        default=None,
        help="an existing `cargo llvm-cov --json` report to read instead of running one",
    )
    parser.add_argument(
        "paths",
        nargs="*",
        help="only these files (paths relative to the repository root)",
    )
    args = parser.parse_args()

    root = repo_root()
    rows = files_of(collect(args.report, root), root)
    if args.paths:
        wanted = {p.rstrip("/") for p in args.paths}
        rows = [r for r in rows if r[0] in wanted or any(r[0].startswith(w + "/") for w in wanted)]
        missing = wanted - {r[0] for r in rows} - {
            w for w in wanted if any(r[0].startswith(w + "/") for r in rows)
        }
        # A file that exists but has no regions — a crate root of `pub mod` and `pub use` — has
        # nothing to cover; only a path that is not there is a mistake.
        no_code = {m for m in missing if os.path.isfile(os.path.join(root, m))}
        for path in sorted(no_code):
            print(f"  {path}  no code (nothing to cover)")
        missing -= no_code
        if missing:
            print(f"no coverage data for: {', '.join(sorted(missing))}", file=sys.stderr)
            return 2

    width = max((len(r[0]) for r in rows), default=10)
    total_covered = sum(r[2] for r in rows)
    total_regions = sum(r[3] for r in rows)
    under = []
    for relative, percent, covered, total in rows:
        flag = " "
        if args.min is not None and percent < args.min and relative not in EXEMPT:
            flag = "!"
            under.append((relative, percent, covered, total))
        print(f"{flag} {relative:<{width}}  {percent:6.2f}%  {covered:>6}/{total:<6}")
    if total_regions:
        print(f"  {'TOTAL':<{width}}  {100.0 * total_covered / total_regions:6.2f}%  "
              f"{total_covered:>6}/{total_regions:<6}")

    if args.min is None:
        return 0
    if not under:
        print(f"\nevery file is at or above {args.min:g}% of regions")
        return 0
    print(f"\n{len(under)} file(s) under {args.min:g}%:")
    for relative, percent, covered, total in under:
        need = math.ceil((args.min / 100.0) * total) - covered
        print(f"  {relative} at {percent:.2f}% — {need} more region(s) to reach {args.min:g}%")
    return 1


if __name__ == "__main__":
    sys.exit(main())
