#!/usr/bin/env python3
"""Regressions and unit tests for scripts/coverage.py.

Verifies:
- Rejection of invalid, empty, or unusable reports (fixing baseline false greens).
- Accurate requested-path handling: unmeasured source files absent from reports must
  fail rather than being falsely labeled as 'no code'.
- Distinguishing explicitly measured zero-region entries from missing data.
- Proper root containment preventing sibling directory pollution.
- Validation of finite threshold in [0, 100].
- Preserving actual low coverage reporting and gate failure exit codes.
- Repository-wide gate failure when existing Rust source files are omitted from reports.
"""

from __future__ import annotations

import io
import json
import math
import os
import pathlib
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

# Ensure scripts directory and repository root are on sys.path
SCRIPTS_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(SCRIPTS_DIR)
if SCRIPTS_DIR not in sys.path:
    sys.path.insert(0, SCRIPTS_DIR)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)

import coverage  # noqa: E402


def run_coverage(args: list[str]) -> tuple[int, str, str]:
    """Invoke coverage.main(args) capturing stdout, stderr, and exit code."""
    out = io.StringIO()
    err = io.StringIO()
    code = 0
    with redirect_stdout(out), redirect_stderr(err):
        try:
            code = coverage.main(args)
        except SystemExit as exc:
            code = exc.code if isinstance(exc.code, int) else 1
    return code, out.getvalue(), err.getvalue()


class TestReportValidation(unittest.TestCase):
    def test_validate_report_rejects_non_dict(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            coverage.validate_report([])  # type: ignore[arg-type]
        self.assertIn("JSON object", str(ctx.exception))

    def test_validate_report_rejects_missing_or_invalid_data(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            coverage.validate_report({})
        self.assertIn("missing 'data' list", str(ctx.exception))

        with self.assertRaises(ValueError) as ctx:
            coverage.validate_report({"data": "not a list"})
        self.assertIn("missing 'data' list", str(ctx.exception))

    def test_validate_report_rejects_empty_data(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            coverage.validate_report({"data": []})
        self.assertIn("'data' is empty", str(ctx.exception))

    def test_validate_report_rejects_empty_files_entries(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            coverage.validate_report({"data": [{"files": []}]})
        self.assertIn("no valid file entries", str(ctx.exception))

    def test_empty_report_fails_gate_instead_of_baseline_false_pass(self) -> None:
        """Baseline bug: {"data": []} exited 0 and said 'every file is at or above 80% of regions'."""
        with tempfile.TemporaryDirectory() as td:
            empty_path = os.path.join(td, "empty.json")
            with open(empty_path, "w", encoding="utf-8") as f:
                json.dump({"data": []}, f)

            code, out, err = run_coverage(["--report", empty_path, "--min", "80"])
            self.assertEqual(code, 2)
            self.assertIn("coverage report 'data' is empty", err)
            self.assertNotIn("every file is at or above 80% of regions", out)

    def test_nonexistent_report_file_fails(self) -> None:
        code, out, err = run_coverage(["--report", "/path/to/nonexistent/report.json", "--min", "80"])
        self.assertEqual(code, 2)
        self.assertIn("cannot read coverage report", err)

    def test_invalid_json_report_file_fails(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            bad_json = os.path.join(td, "bad.json")
            with open(bad_json, "w", encoding="utf-8") as f:
                f.write("{invalid json syntax")

            code, out, err = run_coverage(["--report", bad_json, "--min", "80"])
            self.assertEqual(code, 2)
            self.assertIn("invalid JSON", err)


    def test_malformed_measurements_never_become_zero_region_files(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            source = os.path.join(td, "src.rs")
            pathlib.Path(source).write_text("fn executable() {}\n")
            good = {"filename": source, "summary": {"regions": {"count": 1, "covered": 1}}}
            invalid = [
                {"filename": source},
                {"filename": source, "summary": None},
                {"filename": source, "summary": {"regions": {}}},
            ]
            for total, covered in [(0, None), (None, 0), (-1, 0), (1, -1), (1, 2),
                                   (1.0, 1), (1, float("nan")), (True, 1), (0, 1)]:
                invalid.append({"filename": source, "summary": {
                    "regions": {"count": total, "covered": covered}}})
            report_path = os.path.join(td, "report.json")
            for entry in invalid:
                for entries in [[entry], [good, entry]]:
                    with self.subTest(entries=entries):
                        pathlib.Path(report_path).write_text(json.dumps({"data": [{"files": entries}]}))
                        with mock.patch("coverage.repo_root", return_value=td):
                            code, out, err = run_coverage(["--report", report_path, "--min", "80", "src.rs"])
                        self.assertEqual(code, 2)
                        self.assertIn("region measurements", err)
                        self.assertNotIn("no code", out)
                        self.assertNotIn("every file", out)

    def test_malformed_records_after_a_valid_record_are_rejected(self) -> None:
        good = {"files": [{"filename": "src.rs", "summary": {
            "regions": {"count": 1, "covered": 1}}}]}
        for bad in [None, {"files": None}, {"files": {}}, {"files": [None]},
                    {"files": [{"filename": 123}]}]:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                coverage.validate_report({"data": [good, bad]})



class TestRequestedPathHandling(unittest.TestCase):
    def test_empty_report_with_requested_source_fails_instead_of_no_code(self) -> None:
        """Baseline bug: {"data": []} with refactor.rs exited 0 and labeled it 'no code (nothing to cover)'."""
        with tempfile.TemporaryDirectory() as td:
            empty_path = os.path.join(td, "empty.json")
            with open(empty_path, "w", encoding="utf-8") as f:
                json.dump({"data": []}, f)

            code, out, err = run_coverage([
                "--report", empty_path,
                "--min", "80",
                "crates/prod-code-mcp/src/refactor.rs",
            ])
            self.assertEqual(code, 2)
            self.assertNotIn("no code (nothing to cover)", out)
            self.assertNotIn("every file is at or above", out)

    def test_unmeasured_source_file_missing_from_report_fails(self) -> None:
        """An unmeasured file that exists on disk must fail with exit code 2, not be labeled 'no code'."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            measured_file = os.path.join(src_dir, "measured.rs")
            unmeasured_file = os.path.join(src_dir, "unmeasured.rs")
            with open(measured_file, "w", encoding="utf-8") as f:
                f.write("fn measured() -> bool { true }\n")
            with open(unmeasured_file, "w", encoding="utf-8") as f:
                f.write("fn unmeasured() -> bool { false }\n")

            # Report only contains data for measured.rs, unmeasured.rs is absent
            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": measured_file,
                            "summary": {"regions": {"count": 10, "covered": 10}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                # Request unmeasured.rs which exists on disk but is absent from report
                code, out, err = run_coverage([
                    "--report", report_path,
                    "--min", "80",
                    "src/unmeasured.rs",
                ])
                self.assertEqual(code, 2)
                self.assertIn("no coverage data for: src/unmeasured.rs", err)
                self.assertNotIn("no code (nothing to cover)", out)

    def test_explicitly_measured_zero_region_file_identified_as_no_code(self) -> None:
        """A file that the report explicitly measures with 0 regions is identified as 'no code'."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            zero_reg_file = os.path.join(src_dir, "lib.rs")
            with open(zero_reg_file, "w", encoding="utf-8") as f:
                f.write("pub mod foo;\npub use foo::*;\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": zero_reg_file,
                            "summary": {"regions": {"count": 0, "covered": 0}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage([
                    "--report", report_path,
                    "--min", "80",
                    "src/lib.rs",
                ])
                self.assertEqual(code, 0)
                self.assertIn("src/lib.rs  no code (nothing to cover)", out)
                self.assertIn("every file is at or above 80% of regions", out)

    def test_mixed_zero_region_and_low_coverage_requested_paths(self) -> None:
        """Zero-region file is labeled no code, while low coverage file fails the gate."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            zero_reg_file = os.path.join(src_dir, "lib.rs")
            low_cov_file = os.path.join(src_dir, "low.rs")
            with open(zero_reg_file, "w", encoding="utf-8") as f:
                f.write("pub mod low;\n")
            with open(low_cov_file, "w", encoding="utf-8") as f:
                f.write("fn low() {}\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [
                            {
                                "filename": zero_reg_file,
                                "summary": {"regions": {"count": 0, "covered": 0}},
                            },
                            {
                                "filename": low_cov_file,
                                "summary": {"regions": {"count": 10, "covered": 4}},
                            },
                        ]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage([
                    "--report", report_path,
                    "--min", "80",
                    "src/lib.rs",
                    "src/low.rs",
                ])
                self.assertEqual(code, 1)
                self.assertIn("src/lib.rs  no code (nothing to cover)", out)
                self.assertIn("! src/low.rs", out)
                self.assertIn("1 file(s) under 80%:", out)
                self.assertIn("4 more region(s) to reach 80%", out)


class TestThresholdValidation(unittest.TestCase):
    def test_validate_threshold_valid_values(self) -> None:
        coverage.validate_threshold(None)
        coverage.validate_threshold(0)
        coverage.validate_threshold(0.0)
        coverage.validate_threshold(80.0)
        coverage.validate_threshold(100)
        coverage.validate_threshold(100.0)

    def test_validate_threshold_negative(self) -> None:
        with self.assertRaises(ValueError):
            coverage.validate_threshold(-0.1)
        with self.assertRaises(ValueError):
            coverage.validate_threshold(-50)

    def test_validate_threshold_above_100(self) -> None:
        with self.assertRaises(ValueError):
            coverage.validate_threshold(100.1)
        with self.assertRaises(ValueError):
            coverage.validate_threshold(150)

    def test_validate_threshold_non_finite(self) -> None:
        with self.assertRaises(ValueError):
            coverage.validate_threshold(float("nan"))
        with self.assertRaises(ValueError):
            coverage.validate_threshold(float("inf"))
        with self.assertRaises(ValueError):
            coverage.validate_threshold(float("-inf"))

    def test_cli_rejects_nan_threshold_instead_of_baseline_silent_pass(self) -> None:
        """Baseline bug: --min nan passed because percent < nan was always False."""
        code, out, err = run_coverage(["--min", "nan"])
        self.assertEqual(code, 2)
        self.assertIn("threshold", err)

    def test_cli_rejects_negative_and_out_of_range_thresholds(self) -> None:
        for bad_arg in ["--min=-1", "--min=101", "--min=nan", "--min=inf", "--min=-inf"]:
            code, out, err = run_coverage([bad_arg])
            self.assertEqual(code, 2, f"Expected code 2 for {bad_arg}")
            self.assertIn("threshold", err)

        # Also verify separate arguments where supported by argparse
        for bad_val in ["-1", "101", "nan", "inf"]:
            code, out, err = run_coverage(["--min", bad_val])
            self.assertEqual(code, 2, f"Expected code 2 for --min {bad_val}")


class TestRootContainment(unittest.TestCase):
    def test_is_under_root(self) -> None:
        root = "/workspaces/myrepo"
        # Within root
        self.assertTrue(coverage.is_under_root("/workspaces/myrepo/src/lib.rs", root))
        self.assertTrue(coverage.is_under_root("/workspaces/myrepo/a/b/c.rs", root))

        # Sibling root (baseline bug: string prefix matched "/workspaces/myrepo_sibling")
        self.assertFalse(coverage.is_under_root("/workspaces/myrepo_sibling/src/lib.rs", root))
        self.assertFalse(coverage.is_under_root("/workspaces/myrepo-2/src/lib.rs", root))

        # Parent / outside
        self.assertFalse(coverage.is_under_root("/workspaces/other/lib.rs", root))
        self.assertFalse(coverage.is_under_root("/workspaces/myrepo", root))  # Root itself
        self.assertFalse(coverage.is_under_root("/workspaces/myrepo/../myrepo_sibling/lib.rs", root))

    def test_symlink_beneath_root_does_not_import_external_coverage(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = pathlib.Path(td) / "repo"
            root.mkdir()
            outside = pathlib.Path(td) / "outside.rs"
            outside.write_text("fn external() {}\n")
            (root / "alias.rs").symlink_to(outside)
            self.assertFalse(coverage.is_under_root(str(root / "alias.rs"), str(root)))

    def test_sibling_root_excluded_from_report_parsing(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "target_repo")
            sibling_dir = os.path.join(td, "target_repo_sibling")
            os.makedirs(repo_dir, exist_ok=True)
            os.makedirs(sibling_dir, exist_ok=True)

            sibling_file = os.path.join(sibling_dir, "file.rs")
            with open(sibling_file, "w", encoding="utf-8") as f:
                f.write("fn sibling() {}\n")

            report = {
                "data": [{
                    "files": [{
                        "filename": sibling_file,
                        "summary": {"regions": {"count": 10, "covered": 10}},
                    }]
                }]
            }

            rows, zero_regions = coverage.parse_coverage_entries(report, repo_dir)
            self.assertEqual(rows, [])
            self.assertEqual(zero_regions, set())

    def test_report_containing_only_sibling_root_fails_workspace_gate(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "my_repo")
            sibling_dir = os.path.join(td, "my_repo_sibling")
            os.makedirs(repo_dir, exist_ok=True)
            os.makedirs(sibling_dir, exist_ok=True)

            sibling_file = os.path.join(sibling_dir, "sibling.rs")
            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": sibling_file,
                            "summary": {"regions": {"count": 20, "covered": 20}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80"])
                self.assertEqual(code, 2)
                self.assertIn("no coverage data for repository files in report", err)


class TestActualCoverageReporting(unittest.TestCase):
    def test_actual_low_coverage_fails_with_exact_formatting(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            file_a = os.path.join(src_dir, "a.rs")
            file_b = os.path.join(src_dir, "b.rs")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [
                            {
                                "filename": file_a,
                                "summary": {"regions": {"count": 10, "covered": 5}},
                            },
                            {
                                "filename": file_b,
                                "summary": {"regions": {"count": 20, "covered": 18}},
                            },
                        ]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80"])
                self.assertEqual(code, 1)
                self.assertIn("! src/a.rs", out)
                self.assertIn("  src/b.rs", out)
                self.assertIn("TOTAL", out)
                self.assertIn("1 file(s) under 80%:", out)
                # ceil(0.8 * 10) - 5 = 8 - 5 = 3
                self.assertIn("src/a.rs at 50.00% — 3 more region(s) to reach 80%", out)

    def test_actual_high_coverage_passes(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            file_a = os.path.join(src_dir, "a.rs")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": file_a,
                            "summary": {"regions": {"count": 10, "covered": 9}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80"])
                self.assertEqual(code, 0)
                self.assertIn("src/a.rs", out)
                self.assertIn("every file is at or above 80% of regions", out)

    def test_report_only_mode_without_min(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            file_a = os.path.join(src_dir, "a.rs")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": file_a,
                            "summary": {"regions": {"count": 10, "covered": 5}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path])
                self.assertEqual(code, 0)
                self.assertIn("src/a.rs", out)
                self.assertIn("TOTAL", out)
                self.assertNotIn("every file is at or above", out)
                self.assertNotIn("under", out)


class TestRepositoryWideCoverageGate(unittest.TestCase):
    def test_omitted_source_file_fails_repository_wide_gate(self) -> None:
        """A source file existing in the repository but omitted from the report fails the gate."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            measured_file = os.path.join(src_dir, "measured.rs")
            unmeasured_file = os.path.join(src_dir, "unmeasured.rs")
            with open(measured_file, "w", encoding="utf-8") as f:
                f.write("fn measured() -> bool { true }\n")
            with open(unmeasured_file, "w", encoding="utf-8") as f:
                f.write("fn unmeasured() -> bool { false }\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": measured_file,
                            "summary": {"regions": {"count": 10, "covered": 10}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80"])
                self.assertEqual(code, 2)
                self.assertIn("no coverage data for: src/unmeasured.rs", err)
                self.assertNotIn("every file is at or above", out)

    def test_omitted_source_file_fails_report_only_mode(self) -> None:
        """Report-only mode (no --min) still fails when an existing source file is omitted."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            measured_file = os.path.join(src_dir, "measured.rs")
            unmeasured_file = os.path.join(src_dir, "unmeasured.rs")
            with open(measured_file, "w", encoding="utf-8") as f:
                f.write("fn measured() {}\n")
            with open(unmeasured_file, "w", encoding="utf-8") as f:
                f.write("fn unmeasured() {}\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": measured_file,
                            "summary": {"regions": {"count": 10, "covered": 10}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path])
                self.assertEqual(code, 2)
                self.assertIn("no coverage data for: src/unmeasured.rs", err)

    def test_multiple_omitted_source_files_alphabetically_listed(self) -> None:
        """Multiple omitted files are sorted in the error output."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            measured = os.path.join(src_dir, "m.rs")
            omitted_b = os.path.join(src_dir, "b.rs")
            omitted_a = os.path.join(src_dir, "a.rs")
            with open(measured, "w", encoding="utf-8") as f:
                f.write("fn m() {}\n")
            with open(omitted_b, "w", encoding="utf-8") as f:
                f.write("fn b() {}\n")
            with open(omitted_a, "w", encoding="utf-8") as f:
                f.write("fn a() {}\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": measured,
                            "summary": {"regions": {"count": 10, "covered": 10}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80"])
                self.assertEqual(code, 2)
                self.assertIn("no coverage data for: src/a.rs, src/b.rs", err)

    def test_explicitly_measured_zero_region_file_preserves_pass(self) -> None:
        """Explicitly measured 0-region file passes the repo-wide gate as 'no code'."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            file_a = os.path.join(src_dir, "a.rs")
            zero_file = os.path.join(src_dir, "lib.rs")
            with open(file_a, "w", encoding="utf-8") as f:
                f.write("fn a() {}\n")
            with open(zero_file, "w", encoding="utf-8") as f:
                f.write("pub mod a;\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [
                            {
                                "filename": file_a,
                                "summary": {"regions": {"count": 10, "covered": 10}},
                            },
                            {
                                "filename": zero_file,
                                "summary": {"regions": {"count": 0, "covered": 0}},
                            },
                        ]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80"])
                self.assertEqual(code, 0)
                self.assertIn("src/lib.rs  no code (nothing to cover)", out)
                self.assertIn("src/a.rs", out)
                self.assertIn("every file is at or above 80% of regions", out)

    def test_path_scoped_preserves_behavior_when_unrequested_source_omitted(self) -> None:
        """Path-scoped check succeeds for requested file even if unrequested repo file is omitted."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            os.makedirs(src_dir, exist_ok=True)

            file_a = os.path.join(src_dir, "a.rs")
            omitted = os.path.join(src_dir, "omitted.rs")
            with open(file_a, "w", encoding="utf-8") as f:
                f.write("fn a() {}\n")
            with open(omitted, "w", encoding="utf-8") as f:
                f.write("fn omitted() {}\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": file_a,
                            "summary": {"regions": {"count": 10, "covered": 10}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80", "src/a.rs"])
                self.assertEqual(code, 0)
                self.assertIn("src/a.rs", out)
                self.assertIn("every file is at or above 80% of regions", out)

    def test_integration_tests_directory_not_required_in_report(self) -> None:
        """Integration test files under tests/ are not considered source files to be gated."""
        with tempfile.TemporaryDirectory() as td:
            repo_dir = os.path.join(td, "repo")
            src_dir = os.path.join(repo_dir, "src")
            tests_dir = os.path.join(repo_dir, "tests")
            os.makedirs(src_dir, exist_ok=True)
            os.makedirs(tests_dir, exist_ok=True)

            file_a = os.path.join(src_dir, "a.rs")
            test_file = os.path.join(tests_dir, "integration_test.rs")
            with open(file_a, "w", encoding="utf-8") as f:
                f.write("fn a() {}\n")
            with open(test_file, "w", encoding="utf-8") as f:
                f.write("fn integration_test() {}\n")

            report_path = os.path.join(td, "report.json")
            with open(report_path, "w", encoding="utf-8") as f:
                json.dump({
                    "data": [{
                        "files": [{
                            "filename": file_a,
                            "summary": {"regions": {"count": 10, "covered": 10}},
                        }]
                    }]
                }, f)

            with mock.patch("coverage.repo_root", return_value=repo_dir):
                code, out, err = run_coverage(["--report", report_path, "--min", "80"])
                self.assertEqual(code, 0)
                self.assertIn("src/a.rs", out)
                self.assertIn("every file is at or above 80% of regions", out)


if __name__ == "__main__":
    unittest.main()
