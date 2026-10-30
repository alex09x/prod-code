# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Direct `sed` source-file read detection and scoped one-file fallback test cases."""

from __future__ import annotations

import io
import json
import os
import tempfile
from pathlib import Path
from unittest import mock

from .support import REPO_DIR, GuardTestCaseBase


class SedSourceReadCasesMixin(GuardTestCaseBase):
    cwd: str = ""

    def setUp(self) -> None:
        self.cwd = str(REPO_DIR)

    def test_sed_reads_source_files_are_detected(self) -> None:
        guard = self._guard()
        commands = [
            "sed -n '1,60p' crates/prod-code-mcp/src/tools/schemas/execution.rs",
            "gsed -n '1,60p' /repo/src/main.PY",
            "sed -f filter.sed src/main.rs",
            "sed -e1,40p src/main.rs",
            "sed --expression=1,40p src/main.cpp",
            "env LANG=C sed -e '1,20p' src/main.rs",
            "sudo -u alex09x sed -n '1,20p' src/main.rs",
            "bash -lc 'sed -n \"1,20p\" src/main.rs'",
            "sed -n '1,20p' < src/main.rs",
            "sed -n '1,20p' src/*.rs | head",
        ]
        for command in commands:
            with self.subTest(command=command):
                self.assertIsNotNone(guard.sed_source_target(command, self.cwd))

    def test_extensionless_symlink_to_source_is_detected(self) -> None:
        guard = self._guard()
        with tempfile.TemporaryDirectory() as temp_dir:
            source_dir = Path(temp_dir) / "src"
            source_dir.mkdir()
            source = source_dir / "main.rs"
            source.write_text("fn main() {}\n")
            alias = Path(temp_dir) / "main-source"
            alias.symlink_to(source)

            self.assertIsNotNone(
                guard.sed_source_target("sed -n '1p' main-source", temp_dir)
            )

    def test_clean_language_workspaces_block_sed_expression_reads(self) -> None:
        guard = self._guard()
        cases = [
            ("main.swift", "sed -e1,40p main.swift"),
            ("main.ts", "sed --expression=1,40p main.ts"),
            ("main.py", "sed -e1,40p main.py"),
            ("main.cpp", "sed --expression=1,40p main.cpp"),
        ]
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            self.assertFalse((root / ".git").exists())
            for filename, command in cases:
                (root / filename).write_text("source fixture\n")
                payload = {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Bash",
                    "tool_input": {"command": command},
                    "cwd": temp_dir,
                }
                stdout = io.StringIO()
                with (
                    mock.patch.object(guard, "log"),
                    mock.patch("sys.stdin", io.StringIO(json.dumps(payload))),
                    mock.patch("sys.stdout", stdout),
                ):
                    guard.main()
                response = json.loads(stdout.getvalue())
                self.assertEqual(
                    response["hookSpecificOutput"]["permissionDecision"],
                    "deny",
                    filename,
                )

    def test_documentation_config_stdin_and_output_redirection_are_allowed(
        self,
    ) -> None:
        guard = self._guard()
        commands = [
            "sed -n '1,20p' README.md",
            "sed -n '1,20p' Cargo.toml",
            "sed -n '1,20p' config.yaml",
            "sed -n '1,20p'",
            "printf 'a\\nb\\n' | sed -n '1,2p'",
            "sed -n '1p' <<< 'plain stdin text'",
            "sed -n '1p' < README.md",
            "sed -n '1p' > src/main.rs",
        ]
        for command in commands:
            with self.subTest(command=command):
                self.assertIsNone(guard.sed_source_target(command, self.cwd))

    def test_in_place_edits_are_allowed(self) -> None:
        guard = self._guard()
        commands = [
            "sed -i '' 's/old/new/' src/main.rs",
            "sed -i.bak 's/old/new/' src/main.rs",
            "sed --in-place=.bak 's/old/new/' src/main.rs",
            "sed -ni.bak 's/old/new/' src/main.rs",
        ]
        for command in commands:
            with self.subTest(command=command):
                self.assertIsNone(guard.sed_source_target(command, self.cwd))

    def test_fallback_requires_an_explicit_one_command_opt_in(self) -> None:
        guard = self._guard()
        self.assertTrue(
            guard.source_sed_fallback_requested(
                "PROD_CODE_SOURCE_FALLBACK=1 sed -n '1p' src/main.rs"
            )
        )
        self.assertTrue(
            guard.source_sed_fallback_requested(
                "env PROD_CODE_SOURCE_FALLBACK=1 sed -n '1p' src/main.rs"
            )
        )
        self.assertFalse(
            guard.source_sed_fallback_requested(
                "echo PROD_CODE_SOURCE_FALLBACK=1; sed -n '1p' src/main.rs"
            )
        )
        self.assertFalse(
            guard.source_sed_fallback_requested(
                "PROD_CODE_SOURCE_FALLBACK=1 echo ok; sed -n '1p' src/main.rs"
            )
        )
        with mock.patch.dict(os.environ, {"PROD_CODE_SOURCE_FALLBACK": "1"}):
            self.assertFalse(
                guard.source_sed_fallback_requested("sed -n '1p' src/main.rs")
            )

    def test_fallback_is_limited_to_one_named_source_file(self) -> None:
        guard = self._guard()
        self.assertTrue(
            guard.source_sed_fallback_requested(
                "PROD_CODE_SOURCE_FALLBACK=1 sed -e1,40p src/main.rs", self.cwd
            )
        )
        denied_fallbacks = [
            "PROD_CODE_SOURCE_FALLBACK=1 sed -n '1p' src/*.rs",
            "PROD_CODE_SOURCE_FALLBACK=1 sed -n '1p' src/one.rs src/two.rs",
            "PROD_CODE_SOURCE_FALLBACK=1 sed -n '1p' src/one.rs; sed -n '1p' src/two.rs",
        ]
        for command in denied_fallbacks:
            with self.subTest(command=command):
                self.assertFalse(guard.source_sed_fallback_requested(command, self.cwd))

    def test_claude_pretool_use_denies_source_read_with_semantic_guidance(self) -> None:
        guard = self._guard()
        command = "sed -n '1,60p' src/main.rs"
        payload = {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "cwd": self.cwd,
        }
        stdout = io.StringIO()
        with (
            mock.patch.object(guard, "log"),
            mock.patch("sys.stdin", io.StringIO(json.dumps(payload))),
            mock.patch("sys.stdout", stdout),
            mock.patch.dict(os.environ, {"PROD_CODE_SOURCE_FALLBACK": ""}),
        ):
            guard.main()

        result = json.loads(stdout.getvalue())
        output = result["hookSpecificOutput"]
        self.assertEqual(output["permissionDecision"], "deny")
        reason = output["permissionDecisionReason"]
        self.assertIn("code_outline", reason)
        self.assertIn("code_definition", reason)
        self.assertIn("PROD_CODE_SOURCE_FALLBACK=1", reason)

    def test_claude_pretool_use_logs_explicit_source_fallback(self) -> None:
        guard = self._guard()
        payload = {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": "PROD_CODE_SOURCE_FALLBACK=1 sed -n '1p' src/main.rs"
            },
            "cwd": self.cwd,
        }
        stdout = io.StringIO()
        with (
            mock.patch.object(guard, "log") as log_mock,
            mock.patch("sys.stdin", io.StringIO(json.dumps(payload))),
            mock.patch("sys.stdout", stdout),
            mock.patch.dict(os.environ, {"PROD_CODE_SOURCE_FALLBACK": ""}),
        ):
            guard.main()

        self.assertEqual(stdout.getvalue(), "")
        log_mock.assert_called_once()
        self.assertEqual(log_mock.call_args.args[0]["rule"], "source-sed")
        self.assertEqual(log_mock.call_args.args[0]["decision"], "override")

    def test_claude_pretool_use_denies_glob_fallback(self) -> None:
        guard = self._guard()
        payload = {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": "PROD_CODE_SOURCE_FALLBACK=1 sed -n '1,5p' src/*.rs"
            },
            "cwd": self.cwd,
        }
        stdout = io.StringIO()
        with (
            mock.patch.object(guard, "log"),
            mock.patch("sys.stdin", io.StringIO(json.dumps(payload))),
            mock.patch("sys.stdout", stdout),
        ):
            guard.main()

        response = json.loads(stdout.getvalue())
        self.assertEqual(response["hookSpecificOutput"]["permissionDecision"], "deny")
