# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Symbol-classification and local-build verdict test cases for the guard hook."""

from __future__ import annotations

from .support import GuardTestCaseBase


class ClassificationCasesMixin(GuardTestCaseBase):
    def test_filenames_and_extensions_are_not_symbols(self) -> None:
        guard = self._guard()
        fixture_and_file_names = [
            "meta.json",
            "Cargo.lock",
            "README.md",
            "symbols.csv",
            "config.toml",
            "test.py",
            "main.rs",
            "data.jsonl",
            "schema.sql",
            "docker-compose.yml",
            "settings.ini",
            "error.log",
            "query.graphql",
            "service.proto",
            "icon.png",
            "styles.css",
            "index.html",
        ]
        for name in fixture_and_file_names:
            with self.subTest(name=name):
                self.assertFalse(
                    guard.looks_like_symbol(name),
                    f"{name} should not be classified as a code symbol",
                )

    def test_path_separators_are_not_symbols(self) -> None:
        guard = self._guard()
        paths = [
            "fixtures/meta.json",
            "tests/fixtures/data.csv",
            "./meta.json",
            "path\\to\\file.rs",
        ]
        for path in paths:
            with self.subTest(path=path):
                self.assertFalse(
                    guard.looks_like_symbol(path),
                    f"{path} with path separators should not be classified as a code symbol",
                )

    def test_real_identifiers_are_classified_as_symbols(self) -> None:
        guard = self._guard()
        symbols = [
            "my_func",
            "myMethod",
            "Type::method",
            "crate::module::func",
            "pkg.Func",
            "json.Marshal",
            "models.User",
            "extract_workspace_identifier",
            "node_fast_block_times",
        ]
        for sym in symbols:
            with self.subTest(symbol=sym):
                self.assertTrue(
                    guard.looks_like_symbol(sym),
                    f"{sym} should be classified as a code symbol",
                )

    def test_plain_words_stay_words(self) -> None:
        guard = self._guard()
        words = ["error", "TODO", "Price", "word", "GET"]
        for word in words:
            with self.subTest(word=word):
                self.assertFalse(
                    guard.looks_like_symbol(word),
                    f"{word} should remain a plain word, not a symbol",
                )

    def test_definition_extraction(self) -> None:
        guard = self._guard()
        self.assertEqual(
            guard.symbol_in("fn file_id_for_path"),
            "file_id_for_path",
        )
        self.assertIsNone(guard.symbol_in("meta.json"))
        self.assertIsNone(guard.symbol_in("fn test_"))
        self.assertIsNone(guard.symbol_in("test_foo_"))
        self.assertIsNone(guard.symbol_in("reserve_"))
        self.assertEqual(
            guard.symbol_in("Type::method"),
            "Type::method",
        )


class BuildCommandCasesMixin(GuardTestCaseBase):
    def test_heavy_local_builds_denied(self) -> None:
        guard = self._guard()
        heavy = [
            "cargo test",
            "cargo check",
            "pytest",
            "npm test",
            "swift test",
            "bash -c 'cargo test'",
        ]
        for cmd in heavy:
            with self.subTest(command=cmd):
                self.assertIsNotNone(guard.verdict(cmd))

    def test_allowed_build_exceptions(self) -> None:
        guard = self._guard()
        allowed = [
            "PROD_CODE_LOCAL=1 cargo test",
            "prod-code exec -- cargo test",
            "prod-code shadow-run -- swift test",
            "ssh build-node cargo test",
            "cargo build -p prod-code-client",
            "git status",
            "ls -la",
        ]
        for cmd in allowed:
            with self.subTest(command=cmd):
                self.assertIsNone(guard.verdict(cmd))

    def test_quoted_search_patterns_and_reporting_payloads_allowed_issue_868(self) -> None:
        guard = self._guard()
        # Issue #868: quoted regex alternations and documentation payloads containing build command names
        allowed = [
            "rg -n 'go test ./|cargo test|npm test|swift test' skills -g '*.md'",
            "printf '%s\\n' 'Symptom: ... rg -n '\\''go test ./|cargo test|npm test'\\'''",
            'echo "cargo test and npm test are mentioned in documentation"',
            "cat << 'EOF'\n# Docs\ncargo test\nEOF",
            "cat <<EOF\n# Docs\ncargo test\nEOF",
            "bash -c \"rg -n 'cargo test|swift test' docs/\"",
        ]
        for cmd in allowed:
            with self.subTest(command=cmd):
                self.assertIsNone(
                    guard.verdict(cmd),
                    f"Command with quoted data payload `{cmd}` should be allowed (#868)",
                )

    def test_unquoted_heredoc_command_substitution_denied(self) -> None:
        guard = self._guard()
        cmd = "cat <<EOF\n$(cargo test)\nEOF"
        self.assertIsNotNone(
            guard.verdict(cmd),
            "Command substitution inside unquoted heredoc must be intercepted",
        )

    def test_non_shell_tool_payloads_ignored_issue_868(self) -> None:
        guard = self._guard()
        # Issue #868: apply_patch or other non-shell tools quoting compiler command names
        apply_patch_payload = {
            "tool_name": "apply_patch",
            "tool_input": {
                "cmd": "*** Begin Patch\n*** Add File: test.txt\n+ cargo test\n",
            },
        }
        self.assertIsNone(
            guard.shell_command(apply_patch_payload),
            "apply_patch tool payload should not be treated as a shell command (#868)",
        )

        write_file_payload = {
            "tool_name": "write_to_file",
            "tool_input": {
                "command": "cargo test",
            },
        }
        self.assertIsNone(
            guard.shell_command(write_file_payload),
            "write_to_file tool payload should not be treated as a shell command",
        )

        agy_edit_payload = {
            "toolCall": {
                "name": "replace_file_content",
                "args": {
                    "CommandLine": "cargo test",
                },
            }
        }
        self.assertIsNone(
            guard.shell_command(agy_edit_payload),
            "Non-run_command Antigravity tool should not be treated as a shell command",
        )
