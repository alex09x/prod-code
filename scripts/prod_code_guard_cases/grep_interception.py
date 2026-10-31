# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Grep symbol-interception and shell-payload parsing test cases for the guard hook."""

from __future__ import annotations

import os
import tempfile

from .support import GuardTestCaseBase


class GrepInterceptionCasesMixin(GuardTestCaseBase):
    temp_dir: tempfile.TemporaryDirectory[str] | None = None
    cwd: str = ""

    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.cwd = self.temp_dir.name
        # Initialize a mock git directory with source code folders
        os.makedirs(os.path.join(self.cwd, ".git"), exist_ok=True)
        os.makedirs(os.path.join(self.cwd, "crates"), exist_ok=True)
        os.makedirs(os.path.join(self.cwd, "src"), exist_ok=True)

    def tearDown(self) -> None:
        if self.temp_dir is not None:
            self.temp_dir.cleanup()

    def test_fixture_filename_search_allowed_issue_765(self) -> None:
        guard = self._guard()
        # Issue #765 reproduction: git grep -n -F -e meta.json -- ".rs" ".md"
        cmd = 'git grep -n -F -e meta.json -- ".rs" ".md"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Literal fixture search for meta.json with -F must be allowed (#765)",
        )

    def test_textual_filename_searches_allowed(self) -> None:
        guard = self._guard()
        allowed_searches = [
            'git grep -n "meta.json"',
            'git grep -n "Cargo.lock"',
            'git grep -n "config.toml"',
            'git grep -n -F "fixtures/meta.json"',
            'git grep -n "TODO"',
            'git grep -n "error: unexpected end of file"',
        ]
        for cmd in allowed_searches:
            with self.subTest(command=cmd):
                self.assertIsNone(
                    guard.symbol_grep(cmd, self.cwd),
                    f"Text search `{cmd}` should be allowed without symbol interception",
                )

    def test_fixed_string_workspace_fixture_allowed(self) -> None:
        guard = self._guard()
        fixtures_dir = os.path.join(self.cwd, "tests", "fixtures")
        os.makedirs(fixtures_dir, exist_ok=True)
        fixture_path = os.path.join(fixtures_dir, "custom_fixture_raw")
        with open(fixture_path, "w") as f:
            f.write("test data\n")

        cmd = 'git grep -n -F "custom_fixture_raw" -- "*.rs"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Fixed-string search naming an existing workspace fixture should be allowed",
        )

    def test_identifier_searches_denied(self) -> None:
        guard = self._guard()
        denied_searches = [
            ('git grep -n "my_func" crates/', "my_func"),
            ('git grep -n "fn file_id_for_path" crates/', "file_id_for_path"),
            ('git grep -n "Type::method" src/', "Type::method"),
            ('git grep -n "pkg.Func" .', "pkg.Func"),
            ('git grep -n -F "my_func" crates/', "my_func"),
        ]
        for cmd, expected_symbol in denied_searches:
            with self.subTest(command=cmd):
                res = guard.symbol_grep(cmd, self.cwd)
                self.assertIsNotNone(res, f"Grep command `{cmd}` should be intercepted")
                self.assertEqual(res[0], expected_symbol)

    def test_multiple_patterns_allowed(self) -> None:
        guard = self._guard()
        cmd = 'git grep -n -F -e meta.json -e config.toml -e README.md -- ".rs"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Multiple fixture/filename search patterns should all be allowed",
        )

    def test_case_insensitive_extensions_allowed(self) -> None:
        guard = self._guard()
        cmds = [
            'git grep -n "meta.JSON"',
            'git grep -n "Cargo.LOCK"',
            'git grep -n -F "fixture.YAML"',
        ]
        for cmd in cmds:
            with self.subTest(command=cmd):
                self.assertIsNone(
                    guard.symbol_grep(cmd, self.cwd),
                    f"{cmd} with uppercase extension should be allowed",
                )

    def test_antigravity_and_claude_payload_parsing(self) -> None:
        guard = self._guard()
        # Antigravity format
        agy_payload = {
            "toolCall": {
                "name": "run_command",
                "args": {
                    "CommandLine": 'git grep -n -F -e meta.json -- ".rs"',
                    "Cwd": self.cwd,
                },
            }
        }
        self.assertEqual(
            guard.shell_command(agy_payload),
            'git grep -n -F -e meta.json -- ".rs"',
        )

        # Claude Code format
        claude_payload = {
            "tool_input": {
                "command": 'git grep -n -F -e meta.json -- ".rs"',
            }
        }
        self.assertEqual(
            guard.shell_command(claude_payload),
            'git grep -n -F -e meta.json -- ".rs"',
        )
