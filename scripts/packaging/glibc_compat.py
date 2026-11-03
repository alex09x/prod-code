#!/usr/bin/env python3
#
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
#

"""
glibc_compat.py - Release gate for the glibc ABI that Linux ELF artifacts require.

Reads the ELF version needs (`.gnu.version_r`) with `readelf --version-info`,
including WEAK records, and rejects an artifact when any numeric GLIBC_x.y need is
newer than the explicit baseline. A WEAK need does not stop the loader from
starting the program, but the loader still prints "version `GLIBC_x.y' not
found" on an older glibc, so it fails the gate like any other need. A GLIBC_*
name that is not a plain version number (for example GLIBC_ABI_DT_RELR) cannot be
placed against the baseline and fails conservatively.

The gate only reports; it never hides or filters loader warnings. Anything it
cannot inspect soundly (no readelf, not an ELF file, readelf failure or warnings,
output it cannot account for) is an inspection failure, not a pass.

Exit status: 0 all artifacts pass, 1 an artifact needs a newer or unknown glibc
ABI, 2 an artifact could not be inspected.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass

EXIT_OK = 0
EXIT_INCOMPATIBLE = 1
EXIT_INSPECTION_FAILED = 2

ELF_MAGIC = b"\x7fELF"
NO_VERSION_INFO = "No version information found in this file."
SECTION_RE = re.compile(
    r"^Version needs section '\.gnu\.version_r' contains (\d+) entr(?:y|ies):$"
)
ADDR_RE = re.compile(r"^\s+Addr:\s")
FILE_RE = re.compile(
    r"^\s+(?:0x)?[0-9a-fA-F]+:\s+Version:\s*\d+\s+File:\s*(\S+)\s+Cnt:\s*(\d+)\s*$"
)
NAME_RE = re.compile(
    r"^\s+(?:0x)?[0-9a-fA-F]+:\s+Name:\s*(\S+)\s+Flags:\s*(.*?)\s+Version:\s*\d+\s*$"
)
NUMERIC_GLIBC_RE = re.compile(r"^GLIBC_(\d+(?:\.\d+)+)$")
BASELINE_RE = re.compile(r"^\d+(?:\.\d+)+$")

REBUILD_GUIDANCE = (
    "Do not publish this artifact. Rebuild it on a Linux builder whose glibc is {baseline} "
    "or older (`getconf GNU_LIBC_VERSION` on the builder must print glibc {baseline} or "
    "lower, e.g. an Ubuntu 22.04 builder or container), or link it against a glibc {baseline} "
    "sysroot (e.g. `cargo zigbuild --release --target <arch>-unknown-linux-gnu.{baseline}`), "
    "then rerun this gate. Filtering the loader warning is not a fix."
)


class InspectionError(Exception):
    """The artifact's glibc requirements could not be determined soundly."""


@dataclass(frozen=True)
class VersionNeed:
    library: str
    name: str
    flags: str

    @property
    def weak(self) -> bool:
        return "WEAK" in (token.strip() for token in self.flags.split("|"))

    @property
    def glibc_version(self) -> tuple[int, ...] | None:
        match = NUMERIC_GLIBC_RE.match(self.name)
        return parse_version(match.group(1)) if match else None

    def describe(self) -> str:
        weak = " [WEAK]" if self.weak else ""
        return f"{self.name} from {self.library}{weak}"


def parse_version(text: str) -> tuple[int, ...]:
    return tuple(int(part) for part in text.split("."))


def format_version(version: tuple[int, ...]) -> str:
    return ".".join(str(part) for part in version)


def parse_version_needs(text: str) -> list[VersionNeed]:
    """Parses `readelf --wide --version-info` output into version needs.

    Every record must be accounted for: the entry count in the section header and
    each library's `Cnt` must match the records read, otherwise the output is
    treated as malformed.
    """
    lines = text.splitlines()
    starts = [index for index, line in enumerate(lines) if SECTION_RE.match(line)]
    if not starts:
        if NO_VERSION_INFO in text:
            return []
        raise InspectionError(
            "readelf printed no '.gnu.version_r' section and no 'No version information' line"
        )
    if len(starts) > 1:
        raise InspectionError("readelf printed more than one '.gnu.version_r' section")

    expected_entries = int(SECTION_RE.match(lines[starts[0]]).group(1))
    libraries: list[tuple[str, int]] = []
    needs: list[VersionNeed] = []
    for line in lines[starts[0] + 1 :]:
        if not line.strip() or not line[0].isspace():
            break
        if ADDR_RE.match(line):
            continue
        file_match = FILE_RE.match(line)
        if file_match:
            libraries.append((file_match.group(1), int(file_match.group(2))))
            continue
        name_match = NAME_RE.match(line)
        if name_match and libraries:
            needs.append(
                VersionNeed(
                    libraries[-1][0], name_match.group(1), name_match.group(2).strip()
                )
            )
            continue
        raise InspectionError(
            f"unrecognized line in '.gnu.version_r': {line.strip()!r}"
        )

    if len(libraries) != expected_entries:
        raise InspectionError(
            f"'.gnu.version_r' announces {expected_entries} entries, readelf listed {len(libraries)}"
        )
    for library, count in libraries:
        listed = sum(1 for need in needs if need.library == library)
        if listed != count:
            raise InspectionError(
                f"{library} announces {count} version needs, readelf listed {listed}"
            )
    return needs


def read_elf_needs(path: str, readelf: str) -> list[VersionNeed]:
    """Runs readelf on one ELF artifact; any doubt about the result raises."""
    try:
        with open(path, "rb") as artifact:
            magic = artifact.read(4)
    except OSError as error:
        raise InspectionError(f"cannot read artifact: {error}") from error
    if magic != ELF_MAGIC:
        raise InspectionError(
            "not an ELF file; this gate only inspects Linux ELF artifacts"
        )

    tool = shutil.which(readelf)
    if tool is None:
        raise InspectionError(
            f"'{readelf}' not found; install GNU binutils (readelf) or run the gate on the Linux baseline builder"
        )
    try:
        result = subprocess.run(
            [tool, "--wide", "--version-info", path],
            capture_output=True,
            text=True,
            errors="replace",
            env={**os.environ, "LC_ALL": "C"},
            timeout=120,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise InspectionError(f"readelf could not run: {error}") from error
    stderr = result.stderr.strip()
    if result.returncode != 0:
        raise InspectionError(
            f"readelf exited with status {result.returncode}: {stderr or 'no message'}"
        )
    if stderr:
        raise InspectionError(
            f"readelf reported problems, inspection not trusted: {stderr}"
        )
    return parse_version_needs(result.stdout)


def evaluate(
    path: str, needs: list[VersionNeed], baseline: tuple[int, ...]
) -> tuple[int, list[str]]:
    """Returns the exit status and report lines for one inspected artifact."""
    shown = format_version(baseline)
    glibc = [need for need in needs if need.glibc_version is not None]
    too_new = [need for need in glibc if need.glibc_version > baseline]
    unknown = [
        need
        for need in needs
        if need.name.startswith("GLIBC_") and need.glibc_version is None
    ]

    highest = max((need.glibc_version for need in glibc), default=None)
    summary = (
        f"highest GLIBC need {format_version(highest)}"
        if highest
        else "no GLIBC version needs"
    )
    weak = sorted(
        {need.name for need in glibc if need.weak},
        key=lambda name: parse_version(name[6:]),
    )
    if weak:
        summary += f"; WEAK needs: {', '.join(weak)}"

    if not too_new and not unknown:
        return EXIT_OK, [f"OK   {path}: {summary}; within baseline GLIBC_{shown}"]

    lines = [f"FAIL {path}: {summary}; baseline is GLIBC_{shown}"]
    for need in sorted(too_new, key=lambda item: item.glibc_version):
        line = f"  needs {need.describe()}, newer than GLIBC_{shown}"
        if need.weak:
            line += f'; the loader still warns "version `{need.name}\' not found" on glibc {shown}'
        lines.append(line)
    for need in unknown:
        lines.append(
            f"  needs {need.describe()}: unknown GLIBC ABI requirement, cannot prove it exists in glibc {shown}"
        )
    lines.append("  " + REBUILD_GUIDANCE.format(baseline=shown))
    return EXIT_INCOMPATIBLE, lines


def check_artifacts(
    paths: list[str], baseline: str, readelf: str
) -> tuple[int, list[str]]:
    if not BASELINE_RE.match(baseline):
        raise ValueError(f"baseline must look like 2.35, got {baseline!r}")
    wanted = parse_version(baseline)
    status = EXIT_OK
    report: list[str] = []
    seen: set[str] = set()
    for path in paths:
        key = os.path.realpath(path)
        if key in seen:
            continue
        seen.add(key)
        try:
            needs = read_elf_needs(path, readelf)
        except InspectionError as error:
            status = max(status, EXIT_INSPECTION_FAILED)
            report.append(f"FAIL {path}: inspection failed: {error}")
            continue
        verdict, lines = evaluate(path, needs, wanted)
        status = max(status, verdict)
        report.extend(lines)
    return status, report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Reject Linux ELF release artifacts that need a newer glibc."
    )
    parser.add_argument(
        "--baseline", required=True, help="Oldest supported glibc, e.g. 2.35"
    )
    parser.add_argument(
        "--readelf",
        default=os.environ.get("READELF", "readelf"),
        help="readelf executable (default: $READELF or readelf)",
    )
    parser.add_argument("artifacts", nargs="+", help="Linux ELF artifacts to check")
    args = parser.parse_args(argv)
    try:
        status, report = check_artifacts(args.artifacts, args.baseline, args.readelf)
    except ValueError as error:
        parser.error(str(error))
    for line in report:
        print(f"glibc gate: {line}" if not line.startswith("  ") else line)
    verdict = {
        EXIT_OK: "passed",
        EXIT_INCOMPATIBLE: "FAILED",
        EXIT_INSPECTION_FAILED: "FAILED (inspection)",
    }[status]
    print(f"glibc gate: {verdict} against baseline GLIBC_{args.baseline}")
    return status


if __name__ == "__main__":
    sys.exit(main())
