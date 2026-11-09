#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Regression tests for Mitts CI change scoping and aggregator verification."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from plan import SHARD_SIZE, SKIPPED_MATRIX, VERSION_SMOKE_MATRIX, plan_ci
from scope import (
    affected_workspace_packages,
    parse_lockfile_workspace_versions_only,
    parse_workspace_version_only,
    verify_ci_results,
)


class ScopePlanningTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.com")

    def git(self, *args):
        return subprocess.check_output(
            ["git", *args], cwd=self.root, text=True, stderr=subprocess.PIPE
        ).strip()

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def metadata(self):
        names = [
            ("prod-code-protocol", []),
            ("prod-code-engine-go", ["prod-code-protocol"]),
            ("prod-code-mcp", ["prod-code-protocol"]),
            ("prod-code-client", ["prod-code-mcp", "prod-code-protocol"]),
            (
                "prod-code-gateway",
                ["prod-code-engine-go", "prod-code-mcp", "prod-code-protocol"],
            ),
        ]
        packages = []
        for name, deps in names:
            manifest = self.root / "crates" / name / "Cargo.toml"
            manifest.parent.mkdir(parents=True, exist_ok=True)
            manifest.write_text(f'[package]\nname = "{name}"\n')
            packages.append(
                {
                    "id": name,
                    "name": name,
                    "manifest_path": str(manifest),
                    "dependencies": [{"name": d} for d in deps],
                    "targets": [
                        {"name": name, "kind": ["lib"]},
                        {"name": f"{name}_it", "kind": ["test"]},
                    ],
                }
            )
        return {
            "workspace_members": [n for n, _ in names],
            "packages": packages,
        }

    def baseline_repo(self):
        meta = self.metadata()
        self.write(
            "Cargo.toml",
            '[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "0.3.56"\nedition = "2024"\n',
        )
        self.write(
            "Cargo.lock",
            'version = 4\n\n[[package]]\nname = "prod-code-protocol"\nversion = "0.3.56"\n\n[[package]]\nname = "time"\nversion = "0.3.55"\nsource = "registry"\n',
        )
        self.write("CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n")
        self.git("add", ".")
        self.git("commit", "-m", "baseline")
        return meta, self.git("rev-parse", "HEAD")

    def test_docs_only_change_skips_compilation_jobs(self):
        meta, base = self.baseline_repo()
        self.write("CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n- note\n")
        self.write("docs/guide.md", "# Guide\n")
        self.git("add", ".")
        self.git("commit", "-m", "docs")
        for event in ("pull_request", "push"):
            planned = plan_ci(meta, self.root, event_name=event, base_sha=base)
            self.assertEqual(planned["scope"], "docs")
            self.assertEqual(planned["run_lint"], "false")
            self.assertEqual(planned["run_warm"], "false")
            self.assertEqual(planned["run_tests"], "false")
            self.assertEqual(planned["run_macos"], "false")
            self.assertEqual(planned["matrix"], SKIPPED_MATRIX)

    def test_crate_markdown_fixture_is_not_treated_as_docs_only(self):
        meta, base = self.baseline_repo()
        self.write("crates/prod-code-engine-go/tests/fixture.md", "fixture\n")
        self.git("add", ".")
        self.git("commit", "-m", "crate fixture")
        planned = plan_ci(meta, self.root, event_name="pull_request", base_sha=base)
        self.assertEqual(planned["scope"], "affected")
        self.assertEqual(planned["run_tests"], "true")

    def test_verified_workspace_version_bump_uses_fast_smoke_matrix(self):
        meta, base = self.baseline_repo()
        self.write(
            "Cargo.toml",
            '[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "0.3.57"\nedition = "2024"\n',
        )
        self.write(
            "Cargo.lock",
            'version = 4\n\n[[package]]\nname = "prod-code-protocol"\nversion = "0.3.57"\n\n[[package]]\nname = "time"\nversion = "0.3.55"\nsource = "registry"\n',
        )
        self.write("CHANGELOG.md", "# Changelog\n\n## v0.3.57\n")
        self.git("add", ".")
        self.git("commit", "-m", "release prepare v0.3.57")
        for event in ("pull_request", "push"):
            planned = plan_ci(meta, self.root, event_name=event, base_sha=base)
            self.assertEqual(planned["scope"], "version")
            self.assertEqual(planned["run_lint"], "false")
            self.assertEqual(planned["run_warm"], "false")
            self.assertEqual(planned["run_tests"], "true")
            self.assertEqual(planned["run_macos"], "false")
            self.assertEqual(planned["matrix"], VERSION_SMOKE_MATRIX)

    def test_third_party_lockfile_edit_is_never_classified_as_version_bump(self):
        meta, base = self.baseline_repo()
        self.write(
            "Cargo.lock",
            'version = 4\n\n[[package]]\nname = "prod-code-protocol"\nversion = "0.3.57"\n\n[[package]]\nname = "time"\nversion = "0.3.56"\nsource = "registry"\n',
        )
        self.git("add", ".")
        self.git("commit", "-m", "accidental third-party bump")
        planned = plan_ci(meta, self.root, event_name="pull_request", base_sha=base)
        self.assertEqual(planned["scope"], "full")
        self.assertEqual(planned["run_warm"], "true")

    def test_pull_request_selects_reverse_dependents_including_runtime_binaries(self):
        meta, base = self.baseline_repo()
        self.write("crates/prod-code-engine-go/src/lib.rs", "pub fn go() {}\n")
        self.git("add", ".")
        self.git("commit", "-m", "engine-go edit")
        affected = affected_workspace_packages(
            meta, self.root, {"crates/prod-code-engine-go/src/lib.rs"}
        )
        # engine-go compiles into gateway, whose server binary is spawned by mcp tests;
        # client does not depend on gateway or engine-go.
        self.assertEqual(
            affected,
            {
                "prod-code-engine-go",
                "prod-code-gateway",
                "prod-code-mcp",
            },
        )
        # Editing client selects client + gateway (implicit source_cli), skipping engine-go and protocol.
        client_only = affected_workspace_packages(
            meta, self.root, {"crates/prod-code-client/src/lib.rs"}
        )
        self.assertEqual(client_only, {"prod-code-client", "prod-code-gateway"})
        self.write("crates/prod-code-client/src/lib.rs", "pub fn cli() {}\n")
        self.git("reset", "--hard", base)
        self.write("crates/prod-code-client/src/lib.rs", "pub fn cli() {}\n")
        self.git("add", ".")
        self.git("commit", "-m", "client edit")
        pr_plan = plan_ci(meta, self.root, event_name="pull_request", base_sha=base)
        self.assertEqual(pr_plan["scope"], "affected")
        self.assertEqual(pr_plan["run_macos"], "true")
        pkgs = {r["package"] for r in pr_plan["matrix"]["include"]}
        self.assertEqual(pkgs, {"prod-code-client", "prod-code-gateway"})
        # Push to main for code changes runs full workspace verification.
        push_plan = plan_ci(meta, self.root, event_name="push", base_sha=base)
        self.assertEqual(push_plan["scope"], "full")

    def test_manifest_and_lockfile_parsers_reject_extra_edits(self):
        self.assertFalse(
            parse_workspace_version_only(
                '[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n',
                '[workspace.package]\nversion = "0.1.1"\nedition = "2024"\n',
            )
        )
        self.assertFalse(
            parse_lockfile_workspace_versions_only(
                'version = 4\n\n[[package]]\nname = "ws"\nversion = "0.1.0"\nchecksum = "a"\n',
                'version = 4\n\n[[package]]\nname = "ws"\nversion = "0.1.1"\nchecksum = "b"\n',
                {"ws"},
            )
        )
        self.assertEqual(SHARD_SIZE, 16)

    def test_ci_aggregator_requires_exact_planned_outcomes(self):
        verify_ci_results(
            {
                "plan": {
                    "result": "success",
                    "outputs": {
                        "scope": "affected",
                        "run_lint": "true",
                        "run_warm": "true",
                        "run_tests": "true",
                        "run_macos": "false",
                    },
                },
                "lint": {"result": "success"},
                "warm": {"result": "success"},
                "test": {"result": "success"},
                "macos-signatures": {"result": "skipped"},
            }
        )
        verify_ci_results(
            {
                "plan": {
                    "result": "success",
                    "outputs": {
                        "scope": "docs",
                        "run_lint": "false",
                        "run_warm": "false",
                        "run_tests": "false",
                        "run_macos": "false",
                    },
                },
                "lint": {"result": "skipped"},
                "warm": {"result": "skipped"},
                "test": {"result": "skipped"},
                "macos-signatures": {"result": "skipped"},
            }
        )
        # Empty or malformed outputs must fail closed rather than silently skipping.
        for bad_outputs in (
            {},
            {
                "scope": "docs",
                "run_lint": "",
                "run_warm": "false",
                "run_tests": "false",
                "run_macos": "false",
            },
            {
                "scope": "full",
                "run_lint": "true",
                "run_warm": "true",
                "run_tests": "false",
                "run_macos": "true",
            },
        ):
            with self.assertRaises(AssertionError):
                verify_ci_results(
                    {
                        "plan": {"result": "success", "outputs": bad_outputs},
                        "lint": {"result": "skipped"},
                        "warm": {"result": "skipped"},
                        "test": {"result": "skipped"},
                        "macos-signatures": {"result": "skipped"},
                    }
                )
        with self.assertRaises(AssertionError):
            verify_ci_results(
                {
                    "plan": {
                        "result": "success",
                        "outputs": {
                            "scope": "full",
                            "run_lint": "true",
                            "run_warm": "true",
                            "run_tests": "true",
                            "run_macos": "true",
                        },
                    },
                    "lint": {"result": "success"},
                    "warm": {"result": "success"},
                    "test": {"result": "skipped"},
                    "macos-signatures": {"result": "success"},
                }
            )


if __name__ == "__main__":
    unittest.main()
