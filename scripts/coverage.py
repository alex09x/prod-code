#!/usr/bin/env python3
"""Per-file coverage, and a gate that fails when a file is under the bar.

A workspace total hides exactly what matters: one large untested file drags it down while a
dozen well covered ones hold it up, and the number moves for reasons nobody can point at. This
reports every file and fails on the files themselves.

Run it on a build node, never on the developer's machine:

    prod-code exec --timeout-secs 1800 --no-pull -- \
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
import pathlib
import subprocess
import sys

try:
    import tomllib
except ImportError:
    tomllib = None  # type: ignore[assignment]

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


def validate_threshold(min_coverage: float | None) -> None:
    """Validate that min_coverage is None or a finite number between 0 and 100."""
    if min_coverage is None:
        return
    if not isinstance(min_coverage, (int, float)) or not math.isfinite(min_coverage) or not (0.0 <= min_coverage <= 100.0):
        raise ValueError(f"threshold must be a finite number between 0 and 100, got {min_coverage}")


def parse_threshold(value: str) -> float:
    """Argparse type validator for --min percentage threshold."""
    try:
        val = float(value)
    except (ValueError, TypeError):
        raise argparse.ArgumentTypeError(f"invalid threshold {value!r}: must be a float")
    try:
        validate_threshold(val)
    except ValueError as err:
        raise argparse.ArgumentTypeError(str(err))
    return val


def is_under_root(path: str, root: str) -> bool:
    """Compare complete path components, resolving symlinks on either side."""
    try:
        candidate = pathlib.Path(path).resolve()
        base = pathlib.Path(root).resolve()
        return candidate != base and candidate.is_relative_to(base)
    except (ValueError, TypeError, OSError, RuntimeError):
        return False


def is_test_file(relative: str) -> bool:
    """Check if relative path is inside an integration test directory."""
    parts = pathlib.Path(relative).parts
    return "tests" in parts


def find_rust_source_files(root: str) -> set[str]:
    """Find all Rust source files in the repository/workspace, relative to root."""
    root_path = pathlib.Path(root).resolve()
    cargo_toml = root_path / "Cargo.toml"

    search_dirs: list[pathlib.Path] = []
    excluded_dirs: set[pathlib.Path] = set()

    if cargo_toml.is_file() and tomllib is not None:
        try:
            with open(cargo_toml, "rb") as f:
                manifest: dict[str, object] = tomllib.load(f)
            workspace = manifest.get("workspace")
            if isinstance(workspace, dict):
                excludes: object = workspace.get("exclude", [])
                if isinstance(excludes, list):
                    for exc in excludes:
                        if isinstance(exc, str):
                            for p in root_path.glob(exc):
                                excluded_dirs.add(p.resolve())
                members: object = workspace.get("members", [])
                if isinstance(members, list) and members:
                    for member in members:
                        if isinstance(member, str):
                            for p in root_path.glob(member):
                                if p.is_dir() and p.resolve() not in excluded_dirs:
                                    search_dirs.append(p.resolve())
        except Exception:
            search_dirs = []

    if not search_dirs:
        search_dirs = [root_path]
    search_dirs = list(dict.fromkeys(search_dirs))

    source_files: set[str] = set()
    for base_dir in search_dirs:
        for dirpath, dirnames, filenames in os.walk(base_dir):
            current_path = pathlib.Path(dirpath).resolve()
            if any(current_path == exc or current_path.is_relative_to(exc) for exc in excluded_dirs):
                dirnames.clear()
                continue
            dirnames[:] = [
                d for d in dirnames
                if not d.startswith(".") and d not in ("target", "tests", "benches", "examples")
            ]
            for fname in filenames:
                if not fname.endswith(".rs"):
                    continue
                file_path = (current_path / fname).resolve()
                if not is_under_root(str(file_path), str(root_path)):
                    continue
                try:
                    rel = os.path.relpath(os.path.abspath(file_path), os.path.abspath(root))
                    if rel.startswith(".." + os.sep) or rel == "..":
                        rel = os.path.relpath(os.path.realpath(file_path), os.path.realpath(root))
                except (ValueError, OSError):
                    rel = os.path.relpath(os.path.realpath(file_path), os.path.realpath(root))
                rel = os.path.normpath(rel)
                if is_test_file(rel):
                    continue
                source_files.add(rel)

    return source_files


def validate_report(report: dict) -> None:
    """Require measurements, including explicit zeroes; absence is not zero coverage."""
    if not isinstance(report, dict):
        raise ValueError("coverage report must be a JSON object")
    data = report.get("data")
    if not isinstance(data, list):
        raise ValueError("coverage report missing 'data' list")
    if not data:
        raise ValueError("coverage report 'data' is empty")
    count = 0
    for item in data:
        if not isinstance(item, dict) or not isinstance(item.get("files"), list):
            raise ValueError("each coverage data record must contain a files list")
        for entry in item["files"]:
            if not isinstance(entry, dict):
                raise ValueError("each coverage file entry must be an object")
            filename = entry.get("filename")
            if not isinstance(filename, str) or not filename.strip():
                raise ValueError("coverage file entry has no valid filename")
            summary = entry.get("summary")
            regions = summary.get("regions") if isinstance(summary, dict) else None
            if not isinstance(regions, dict):
                raise ValueError(f"missing region measurements for {filename}")
            total, covered = regions.get("count"), regions.get("covered")
            if (type(total) is not int or type(covered) is not int
                    or total < 0 or covered < 0 or covered > total):
                raise ValueError(f"invalid region measurements for {filename}")
            count += 1
    if not count:
        raise ValueError("coverage report contains no valid file entries")


def collect(report_path: str | None, root: str) -> dict:
    if report_path:
        try:
            with open(report_path, "r", encoding="utf-8") as handle:
                report = json.load(handle)
        except OSError as err:
            raise ValueError(f"cannot read coverage report {report_path}: {err}") from err
        except json.JSONDecodeError as err:
            raise ValueError(f"invalid JSON in coverage report {report_path}: {err}") from err
        validate_report(report)
        return report
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
    try:
        with open(out_path, "r", encoding="utf-8") as handle:
            report = json.load(handle)
    except (OSError, json.JSONDecodeError) as err:
        raise ValueError(f"cannot read generated coverage report {out_path}: {err}") from err
    validate_report(report)
    return report


def parse_coverage_entries(
    report: dict, root: str
) -> tuple[list[tuple[str, float, int, int]], set[str]]:
    """Return (rows, zero_region_files) for files in report within root.
    rows contains (relative_path, coverage_percent, covered_regions, total_regions) where total > 0.
    zero_region_files contains relative paths of files explicitly measured with total == 0 regions.
    """
    rows: list[tuple[str, float, int, int]] = []
    zero_regions: set[str] = set()
    for data in report.get("data", []):
        if not isinstance(data, dict):
            continue
        for entry in data.get("files", []):
            if not isinstance(entry, dict):
                continue
            path = entry.get("filename", "")
            if not path:
                continue
            abs_path = path if os.path.isabs(path) else os.path.join(root, path)
            if not is_under_root(abs_path, root):
                continue
            try:
                relative = os.path.relpath(os.path.abspath(abs_path), os.path.abspath(root))
                if relative.startswith(".." + os.sep) or relative == "..":
                    relative = os.path.relpath(os.path.realpath(abs_path), os.path.realpath(root))
            except (ValueError, OSError):
                relative = os.path.relpath(os.path.realpath(abs_path), os.path.realpath(root))
            relative = os.path.normpath(relative)
            if is_test_file(relative):
                continue
            regions = entry.get("summary", {}).get("regions", {})
            total = regions.get("count", 0)
            covered = regions.get("covered", 0)
            if total == 0:
                zero_regions.add(relative)
                continue
            zero_regions.discard(relative)
            rows.append((relative, 100.0 * covered / total, covered, total))
    rows.sort(key=lambda row: row[1])
    return rows, zero_regions


def files_of(report: dict, root: str) -> list[tuple[str, float, int, int]]:
    """(path relative to the repository, region coverage, covered regions, total regions)."""
    rows, _ = parse_coverage_entries(report, root)
    return rows


def zero_region_files_of(report: dict, root: str) -> set[str]:
    """Paths relative to repository that are in the report with 0 total regions."""
    _, zero_regions = parse_coverage_entries(report, root)
    return zero_regions


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--min",
        type=parse_threshold,
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
    args = parser.parse_args(argv)

    if args.min is not None:
        try:
            validate_threshold(args.min)
        except ValueError as err:
            print(f"invalid threshold: {err}", file=sys.stderr)
            return 2

    root = repo_root()
    try:
        report = collect(args.report, root)
    except (ValueError, subprocess.CalledProcessError) as err:
        print(f"coverage report error: {err}", file=sys.stderr)
        return 2

    rows, zero_region_files = parse_coverage_entries(report, root)

    if args.paths:
        wanted = {os.path.normpath(p.rstrip("/").rstrip(os.sep)) for p in args.paths}
        matched_rows = [
            r for r in rows
            if r[0] in wanted or any(r[0].startswith(w + os.sep) for w in wanted)
        ]
        matched_wanted_rows = {
            w for w in wanted
            if any(r[0] == w or r[0].startswith(w + os.sep) for r in matched_rows)
        }
        no_code = {
            z for z in zero_region_files
            if z in wanted or any(z.startswith(w + os.sep) for w in wanted)
        }
        for path in sorted(no_code):
            print(f"  {path}  no code (nothing to cover)")

        matched_wanted_no_code = {
            w for w in wanted
            if w in no_code or any(z.startswith(w + os.sep) for z in no_code)
        }
        missing = wanted - matched_wanted_rows - matched_wanted_no_code
        if missing:
            print(f"no coverage data for: {', '.join(sorted(missing))}", file=sys.stderr)
            return 2

        rows = matched_rows
    else:
        if not rows and not zero_region_files:
            print("no coverage data for repository files in report", file=sys.stderr)
            return 2

        repo_files = find_rust_source_files(root)
        report_files = {r[0] for r in rows} | zero_region_files
        missing = repo_files - report_files
        if missing:
            print(f"no coverage data for: {', '.join(sorted(missing))}", file=sys.stderr)
            return 2

        for path in sorted(zero_region_files):
            print(f"  {path}  no code (nothing to cover)")

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
