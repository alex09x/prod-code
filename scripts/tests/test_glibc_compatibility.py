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

"""Tests for the Linux release glibc baseline gate (scripts/packaging/glibc_compat.py)."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import shutil
import stat
import sys
import tempfile
import unittest
from pathlib import Path

PACKAGING = Path(__file__).resolve().parent.parent / "packaging"
_SPEC = importlib.util.spec_from_file_location(
    "glibc_compat", PACKAGING / "glibc_compat.py"
)
gate = importlib.util.module_from_spec(_SPEC)
sys.modules["glibc_compat"] = gate
_SPEC.loader.exec_module(gate)

HEADER = " Addr: 0x0000000000001930  Offset: 0x00001930  Link: 8 (.dynstr)"


def needs_output(
    *libraries: tuple[str, list[tuple[str, str]]], announced: int | None = None
) -> str:
    """Builds `readelf --wide --version-info` text in GNU binutils layout."""
    count = len(libraries) if announced is None else announced
    lines = [
        f"Version needs section '.gnu.version_r' contains {count} entries:",
        HEADER,
    ]
    offset = 0
    for library, names in libraries:
        lines.append(f"  {offset:06x}: Version: 1  File: {library}  Cnt: {len(names)}")
        for name, flags in names:
            offset += 0x10
            lines.append(
                f"  0x{offset:04x}:   Name: {name}  Flags: {flags}  Version: 2"
            )
    return "\n".join(lines) + "\n"


# Shaped like the v0.3.49 x86_64 server: strict needs stop at 2.34, one WEAK 2.39 need.
V49_LIKE = needs_output(
    ("libgcc_s.so.1", [("GCC_3.0", "none")]),
    (
        "libc.so.6",
        [
            ("GLIBC_2.2.5", "none"),
            ("GLIBC_2.30", "WEAK"),
            ("GLIBC_2.34", "none"),
            ("GLIBC_2.39", "WEAK"),
        ],
    ),
)


class GateTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="glibc-gate-"))
        self.addCleanup(shutil.rmtree, self.tmp)
        self.elf = self.tmp / "prod-code-server"
        self.elf.write_bytes(gate.ELF_MAGIC + b"\x02\x01\x01" + b"\x00" * 57)

    def fake_readelf(self, stdout: str, stderr: str = "", status: int = 0) -> str:
        (self.tmp / "out.txt").write_text(stdout, encoding="utf-8")
        (self.tmp / "err.txt").write_text(stderr, encoding="utf-8")
        script = self.tmp / "readelf"
        script.write_text(
            f"#!/bin/sh\ncat '{self.tmp}/out.txt'\ncat '{self.tmp}/err.txt' >&2\nexit {status}\n",
            encoding="utf-8",
        )
        script.chmod(script.stat().st_mode | stat.S_IXUSR)
        return str(script)

    def run_gate(self, readelf: str, *paths: str) -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            status = gate.main(
                [
                    "--baseline",
                    "2.35",
                    "--readelf",
                    readelf,
                    *(paths or [str(self.elf)]),
                ]
            )
        return status, out.getvalue()


class VersionNeedTests(GateTestCase):
    def test_weak_need_above_baseline_fails_with_rebuild_guidance(self) -> None:
        status, report = self.run_gate(self.fake_readelf(V49_LIKE))
        self.assertEqual(status, gate.EXIT_INCOMPATIBLE, report)
        self.assertIn(
            "needs GLIBC_2.39 from libc.so.6 [WEAK], newer than GLIBC_2.35", report
        )
        self.assertIn("the loader still warns", report)
        self.assertIn("getconf GNU_LIBC_VERSION", report)
        self.assertIn("Filtering the loader warning is not a fix", report)
        self.assertNotIn("GLIBC_2.34 from", report)

    def test_needs_within_baseline_pass(self) -> None:
        output = needs_output(
            (
                "libc.so.6",
                [
                    ("GLIBC_2.2.5", "none"),
                    ("GLIBC_2.30", "WEAK"),
                    ("GLIBC_2.35", "none"),
                ],
            )
        )
        status, report = self.run_gate(self.fake_readelf(output))
        self.assertEqual(status, gate.EXIT_OK, report)
        self.assertIn("highest GLIBC need 2.35; WEAK needs: GLIBC_2.30", report)

    def test_unknown_glibc_abi_requirement_fails_conservatively(self) -> None:
        output = needs_output(
            ("libc.so.6", [("GLIBC_2.17", "none"), ("GLIBC_ABI_DT_RELR", "none")])
        )
        status, report = self.run_gate(self.fake_readelf(output))
        self.assertEqual(status, gate.EXIT_INCOMPATIBLE, report)
        self.assertIn(
            "GLIBC_ABI_DT_RELR from libc.so.6: unknown GLIBC ABI requirement", report
        )

    def test_other_library_versions_are_not_glibc_needs(self) -> None:
        output = needs_output(
            ("libstdc++.so.6", [("GLIBCXX_3.4.32", "none")]),
            ("libgcc_s.so.1", [("GCC_14.0", "none")]),
        )
        status, report = self.run_gate(self.fake_readelf(output))
        self.assertEqual(status, gate.EXIT_OK, report)
        self.assertIn("no GLIBC version needs", report)


class InspectionFailureTests(GateTestCase):
    def test_count_mismatch_is_malformed(self) -> None:
        status, report = self.run_gate(
            self.fake_readelf(
                needs_output(("libc.so.6", [("GLIBC_2.2.5", "none")]), announced=2)
            )
        )
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)
        self.assertIn("announces 2 entries", report)

    def test_unrecognized_record_is_malformed(self) -> None:
        broken = V49_LIKE.replace(
            "Name: GLIBC_2.39  Flags: WEAK", "Nom: GLIBC_2.39  Flags: WEAK"
        )
        status, report = self.run_gate(self.fake_readelf(broken))
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)
        self.assertIn("unrecognized line", report)

    def test_missing_needs_section_is_not_a_pass(self) -> None:
        status, report = self.run_gate(
            self.fake_readelf(
                "Version symbols section '.gnu.version' contains 3 entries:\n"
            )
        )
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)

    def test_readelf_failure_is_reported(self) -> None:
        status, report = self.run_gate(
            self.fake_readelf(
                "", "readelf: Error: Failed to read file's magic number\n", 1
            )
        )
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)
        self.assertIn(
            "exited with status 1: readelf: Error: Failed to read file's magic number",
            report,
        )

    def test_readelf_warning_is_surfaced_not_suppressed(self) -> None:
        warning = "readelf: Warning: corrupt version need entry"
        status, report = self.run_gate(
            self.fake_readelf(
                V49_LIKE.replace("GLIBC_2.39", "GLIBC_2.31"), warning + "\n"
            )
        )
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)
        self.assertIn(warning, report)

    def test_non_elf_artifact_fails_without_readelf(self) -> None:
        macho = self.tmp / "prod-code-macos"
        macho.write_bytes(b"\xcf\xfa\xed\xfe" + b"\x00" * 60)
        status, report = self.run_gate(str(self.tmp / "no-such-readelf"), str(macho))
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)
        self.assertIn("not an ELF file", report)

    def test_missing_readelf_fails(self) -> None:
        status, report = self.run_gate(str(self.tmp / "no-such-readelf"))
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)
        self.assertIn("not found", report)

    def test_any_failure_wins_over_passing_artifacts(self) -> None:
        missing = str(self.tmp / "missing-binary")
        status, report = self.run_gate(
            self.fake_readelf(V49_LIKE), missing, str(self.elf)
        )
        self.assertEqual(status, gate.EXIT_INSPECTION_FAILED, report)
        self.assertIn("cannot read artifact", report)


class RealElfTests(unittest.TestCase):
    @unittest.skipUnless(
        sys.platform.startswith("linux") and shutil.which("readelf"),
        "needs Linux and readelf",
    )
    def test_real_interpreter_is_parsed_and_judged(self) -> None:
        interpreter = os.path.realpath(sys.executable)
        status, report = gate.check_artifacts([interpreter], "2.0", "readelf")
        self.assertEqual(status, gate.EXIT_INCOMPATIBLE, report)
        status, report = gate.check_artifacts([interpreter], "999.0", "readelf")
        self.assertEqual(status, gate.EXIT_OK, report)


class PackagingWiringTests(unittest.TestCase):
    def test_gate_runs_on_linux_binaries_before_any_linux_package(self) -> None:
        script = (PACKAGING / "build-all-packages.sh").read_text(encoding="utf-8")
        gate_call = script.index(
            'python3 "$SCRIPT_DIR/glibc_compat.py" --baseline "$LINUX_GLIBC_BASELINE"'
        )
        self.assertIn('LINUX_GLIBC_BASELINE="2.35"', script)
        self.assertLess(gate_call, script.index("package-deb.py"))
        self.assertLess(gate_call, script.index("package-macos-pkg.sh"))
        gated = script[script.index("LINUX_RELEASE_BINS=(") : gate_call]
        self.assertIn("prod-code-server", gated)
        self.assertNotIn('"$CLIENT_BIN"', gated)


if __name__ == "__main__":
    unittest.main()
