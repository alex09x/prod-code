#!/usr/bin/env python3
"""Regression tests for prod-code agent guard hook (scripts/hooks/prod-code-local-build-guard.py)."""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

HOOK_SCRIPT = Path(__file__).parent / "hooks" / "prod-code-local-build-guard.py"

spec = importlib.util.spec_from_file_location("guard", str(HOOK_SCRIPT))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class ProdCodeGuardClassificationTests(unittest.TestCase):
    def test_filenames_and_extensions_are_not_symbols(self) -> None:
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
        words = ["error", "TODO", "Price", "word", "GET"]
        for word in words:
            with self.subTest(word=word):
                self.assertFalse(
                    guard.looks_like_symbol(word),
                    f"{word} should remain a plain word, not a symbol",
                )

    def test_definition_extraction(self) -> None:
        self.assertEqual(
            guard.symbol_in("fn file_id_for_path"),
            "file_id_for_path",
        )
        self.assertIsNone(guard.symbol_in("meta.json"))
        self.assertEqual(
            guard.symbol_in("Type::method"),
            "Type::method",
        )


class ProdCodeGuardGrepInterceptionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.cwd = self.temp_dir.name
        # Initialize a mock git directory with source code folders
        os.makedirs(os.path.join(self.cwd, ".git"), exist_ok=True)
        os.makedirs(os.path.join(self.cwd, "crates"), exist_ok=True)
        os.makedirs(os.path.join(self.cwd, "src"), exist_ok=True)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_fixture_filename_search_allowed_issue_765(self) -> None:
        # Issue #765 reproduction: git grep -n -F -e meta.json -- ".rs" ".md"
        cmd = 'git grep -n -F -e meta.json -- ".rs" ".md"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Literal fixture search for meta.json with -F must be allowed (#765)",
        )

    def test_textual_filename_searches_allowed(self) -> None:
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
        cmd = 'git grep -n -F -e meta.json -e config.toml -e README.md -- ".rs"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Multiple fixture/filename search patterns should all be allowed",
        )

    def test_case_insensitive_extensions_allowed(self) -> None:
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


class ProdCodeGuardBuildCommandTests(unittest.TestCase):
    def test_heavy_local_builds_denied(self) -> None:
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
        allowed = [
            "PROD_CODE_LOCAL=1 cargo test",
            "prod-code exec -- cargo test",
            "prod-code shadow-run -- swift test",
            "ssh booster cargo test",
            "cargo build -p prod-code-client",
            "git status",
            "ls -la",
        ]
        for cmd in allowed:
            with self.subTest(command=cmd):
                self.assertIsNone(guard.verdict(cmd))

    def test_quoted_search_patterns_and_reporting_payloads_allowed_issue_868(self) -> None:
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
        cmd = "cat <<EOF\n$(cargo test)\nEOF"
        self.assertIsNotNone(
            guard.verdict(cmd),
            "Command substitution inside unquoted heredoc must be intercepted",
        )

    def test_non_shell_tool_payloads_ignored_issue_868(self) -> None:
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


if __name__ == "__main__":
    unittest.main()
