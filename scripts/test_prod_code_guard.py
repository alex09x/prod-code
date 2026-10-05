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
"""Regression tests for prod-code agent guard hook (scripts/hooks/prod-code-local-build-guard.py)."""

from __future__ import annotations

import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest import mock

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


class ProdCodeGuardMultiFileRenameTests(unittest.TestCase):
    def test_single_symbol_rename_detection(self) -> None:
        self.assertEqual(
            guard.single_symbol_rename("old_handler", "new_handler"),
            ("old_handler", "new_handler"),
        )
        self.assertEqual(
            guard.single_symbol_rename("OldStruct", "NewStruct"),
            ("OldStruct", "NewStruct"),
        )
        self.assertEqual(
            guard.single_symbol_rename("var1", "var2"),
            ("var1", "var2"),
        )
        # Keywords rejected
        for kw in ("fn", "func", "let", "mut", "var", "const", "class", "struct", "type", "return", "if", "switch"):
            with self.subTest(keyword=kw):
                self.assertIsNone(guard.single_symbol_rename(kw, "other"))
                self.assertIsNone(guard.single_symbol_rename("other", kw))
        # Non-identifiers rejected
        self.assertIsNone(guard.single_symbol_rename("123", "456"))
        self.assertIsNone(guard.single_symbol_rename("foo-bar", "baz"))
        self.assertIsNone(guard.single_symbol_rename("foo.bar", "baz"))
        self.assertIsNone(guard.single_symbol_rename("a", "b"))  # too short (< 2)
        # Contextual single-symbol renames (call-sites, definitions, imports)
        self.assertEqual(
            guard.single_symbol_rename(
                "        let hash1 = fnv1a_64(text1.as_bytes());",
                "        let hash1 = normalized_source_hash(text1.as_bytes());",
            ),
            ("fnv1a_64", "normalized_source_hash"),
        )
        self.assertEqual(
            guard.single_symbol_rename(
                "use ridx_format::{fnv1a_64, DefKind};",
                "use ridx_format::{normalized_source_hash, DefKind};",
            ),
            ("fnv1a_64", "normalized_source_hash"),
        )
        self.assertEqual(
            guard.single_symbol_rename(
                "pub fn fnv1a_64(&self)",
                "pub fn normalized_source_hash(&self)",
            ),
            ("fnv1a_64", "normalized_source_hash"),
        )
        # Multi-line code / statements with logic changes rejected
        self.assertIsNone(guard.single_symbol_rename("let x = foo();", "let x = foo() + 10;"))
        self.assertIsNone(guard.single_symbol_rename("if x > 0 { return true; }", "if x >= 0 { return false; }"))
        self.assertIsNone(guard.single_symbol_rename("let a = 1; let b = 2;", "let c = 1; let d = 2;"))
        self.assertEqual(guard.single_symbol_rename("fn old() {}", "fn new() {}"), ("old", "new"))
        self.assertIsNone(guard.single_symbol_rename("fn old() { 1 }", "fn old() { 2 }"))
        # Identical or empty
        self.assertIsNone(guard.single_symbol_rename("foo", "foo"))
        self.assertIsNone(guard.single_symbol_rename("", "bar"))
        self.assertIsNone(guard.single_symbol_rename("foo", ""))

    def test_multi_file_rename_state_tracking(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            orig_dir = guard.RENAME_STATE_DIR
            try:
                guard.RENAME_STATE_DIR = tmpdir
                conv = "test-conv-123"

                # 1st file: allowed
                denied, other = guard.check_multi_file_rename(conv, "/repo/src/a.rs", "old_func", "new_func", repo_root="/repo")
                self.assertFalse(denied)
                self.assertIsNone(other)

                # Verify file permissions mode 0600
                state_file = os.path.join(tmpdir, f"rename_{conv}_repo.json")
                self.assertTrue(os.path.exists(state_file))
                file_mode = os.stat(state_file).st_mode & 0o777
                self.assertEqual(file_mode, 0o600)

                # Same file again: allowed
                denied, other = guard.check_multi_file_rename(conv, "/repo/src/a.rs", "old_func", "new_func", repo_root="/repo")
                self.assertFalse(denied)
                self.assertIsNone(other)

                # Different rename target for same old symbol (config -> cfg vs config -> settings): allowed!
                denied, other = guard.check_multi_file_rename(conv, "/repo/src/b.rs", "old_func", "distinct_target", repo_root="/repo")
                self.assertFalse(denied)
                self.assertIsNone(other)

                # Different repository for same (old_sym, new_sym): allowed!
                denied, other = guard.check_multi_file_rename(conv, "/other_repo/src/b.rs", "old_func", "new_func", repo_root="/other_repo")
                self.assertFalse(denied)
                self.assertIsNone(other)

                # Different file with same (old_sym, new_sym) in SAME repo: denied!
                denied, other = guard.check_multi_file_rename(conv, "/repo/src/b.rs", "old_func", "new_func", repo_root="/repo")
                self.assertTrue(denied)
                self.assertEqual(other, os.path.abspath("/repo/src/a.rs"))

                # Stale session file expiration (>1800s)
                stale_file = os.path.join(tmpdir, "rename_old_session.json")
                stale_lock = os.path.join(tmpdir, "rename_old_session.lock")
                with open(stale_file, "w") as f:
                    f.write("{}")
                with open(stale_lock, "w") as f:
                    f.write("")
                past_time = int(time.time()) - 2000
                os.utime(stale_file, (past_time, past_time))
                os.utime(stale_lock, (past_time, past_time))
                # Next check cleans up stale .json file but PRESERVES .lock file
                guard.check_multi_file_rename(conv, "/repo/src/c.rs", "foo", "bar", repo_root="/repo")
                self.assertFalse(os.path.exists(stale_file))
                self.assertTrue(os.path.exists(stale_lock), "Lock files must never be age-unlinked to prevent split inode races")

                # Lock acquisition failure fails safely (returns False, None) without exception
                with mock.patch("fcntl.flock", side_effect=OSError("flock failed")):
                    denied, other = guard.check_multi_file_rename(conv, "/repo/src/d.rs", "foo", "bar", repo_root="/repo")
                    self.assertFalse(denied)
                    self.assertIsNone(other)
            finally:
                guard.RENAME_STATE_DIR = orig_dir

    def test_file_edit_details_extraction(self) -> None:
        # Antigravity format
        agy_payload = {
            "toolCall": {
                "name": "replace_file_content",
                "args": {
                    "TargetFile": "/repo/src/main.rs",
                    "TargetContent": "old_var",
                    "ReplacementContent": "new_var",
                    "Instruction": "Rename variable",
                    "Description": "Renaming",
                },
            }
        }
        res = guard.file_edit_details(agy_payload)
        self.assertIsNotNone(res)
        file_path, old_c, new_c, override = res
        self.assertEqual(file_path, "/repo/src/main.rs")
        self.assertEqual(old_c, "old_var")
        self.assertEqual(new_c, "new_var")
        self.assertFalse(override)

        # Override format in Antigravity Instruction
        agy_override = {
            "toolCall": {
                "name": "replace_file_content",
                "args": {
                    "TargetFile": "/repo/src/main.rs",
                    "TargetContent": "old_var",
                    "ReplacementContent": "new_var",
                    "Instruction": "PROD_CODE_MANUAL_RENAME=1 rename manually",
                    "Description": "Renaming",
                },
            }
        }
        res = guard.file_edit_details(agy_override)
        self.assertIsNotNone(res)
        _, _, _, override = res
        self.assertTrue(override)

        # Claude Code format
        claude_payload = {
            "tool_name": "Edit",
            "tool_input": {
                "file_path": "/repo/src/lib.rs",
                "old_string": "old_foo",
                "new_string": "new_foo",
            },
        }
        res = guard.file_edit_details(claude_payload)
        self.assertIsNotNone(res)
        file_path, old_c, new_c, override = res
        self.assertEqual(file_path, "/repo/src/lib.rs")
        self.assertEqual(old_c, "old_foo")
        self.assertEqual(new_c, "new_foo")
        self.assertFalse(override)

        # Claude Code format with override in tool_input explanation
        claude_override = {
            "tool_name": "Edit",
            "tool_input": {
                "file_path": "/repo/src/lib.rs",
                "old_string": "old_foo",
                "new_string": "new_foo",
                "explanation": "PROD_CODE_MANUAL_RENAME=1 template fallback",
            },
        }
        res = guard.file_edit_details(claude_override)
        self.assertIsNotNone(res)
        _, _, _, override = res
        self.assertTrue(override)

        # Environment variable override for any agent
        orig_env = os.environ.get("PROD_CODE_MANUAL_RENAME")
        try:
            os.environ["PROD_CODE_MANUAL_RENAME"] = "1"
            res = guard.file_edit_details(claude_payload)
            self.assertIsNotNone(res)
            _, _, _, override = res
            self.assertTrue(override)
        finally:
            if orig_env is None:
                os.environ.pop("PROD_CODE_MANUAL_RENAME", None)
            else:
                os.environ["PROD_CODE_MANUAL_RENAME"] = orig_env

    def test_end_to_end_antigravity_rename_guard(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            orig_dir = guard.RENAME_STATE_DIR
            try:
                guard.RENAME_STATE_DIR = tmpdir
                conv = "conv-e2e"

                # Edit 1 on file a.rs (inside git repo)
                payload_1 = {
                    "toolCall": {
                        "name": "replace_file_content",
                        "args": {
                            "TargetFile": str(Path(__file__).parent / "hooks" / "file_a.py"),
                            "TargetContent": "my_symbol",
                            "ReplacementContent": "my_new_symbol",
                            "Instruction": "rename",
                            "Description": "rename",
                        },
                    },
                    "conversationId": conv,
                    "workspacePaths": [str(Path(__file__).parent.parent)],
                }
                stdout_1 = io.StringIO()
                orig_stdin, orig_stdout = sys.stdin, sys.stdout
                try:
                    sys.stdin = io.StringIO(json.dumps(payload_1))
                    sys.stdout = stdout_1
                    guard.main()
                finally:
                    sys.stdin, sys.stdout = orig_stdin, orig_stdout
                self.assertEqual(stdout_1.getvalue().strip(), "")

                # Edit 2 on file b.py with SAME symbol -> should be DENIED
                payload_2 = {
                    "toolCall": {
                        "name": "replace_file_content",
                        "args": {
                            "TargetFile": str(Path(__file__).parent / "hooks" / "file_b.py"),
                            "TargetContent": "my_symbol",
                            "ReplacementContent": "my_new_symbol",
                            "Instruction": "rename",
                            "Description": "rename",
                        },
                    },
                    "conversationId": conv,
                    "workspacePaths": [str(Path(__file__).parent.parent)],
                }
                stdout_2 = io.StringIO()
                try:
                    sys.stdin = io.StringIO(json.dumps(payload_2))
                    sys.stdout = stdout_2
                    guard.main()
                finally:
                    sys.stdin, sys.stdout = orig_stdin, orig_stdout
                out_json = json.loads(stdout_2.getvalue())
                self.assertEqual(out_json.get("decision"), "deny")
                self.assertIn("code_rename", out_json.get("reason", ""))
                self.assertIn("file_a.py", out_json.get("reason", ""))

                # Edit 3 on file c.py with SAME symbol but PROD_CODE_MANUAL_RENAME=1 override -> ALLOWED
                payload_3 = {
                    "toolCall": {
                        "name": "replace_file_content",
                        "args": {
                            "TargetFile": str(Path(__file__).parent / "hooks" / "file_c.py"),
                            "TargetContent": "my_symbol",
                            "ReplacementContent": "my_new_symbol",
                            "Instruction": "PROD_CODE_MANUAL_RENAME=1",
                            "Description": "manual rename override",
                        },
                    },
                    "conversationId": conv,
                    "workspacePaths": [str(Path(__file__).parent.parent)],
                }
                stdout_3 = io.StringIO()
                try:
                    sys.stdin = io.StringIO(json.dumps(payload_3))
                    sys.stdout = stdout_3
                    guard.main()
                finally:
                    sys.stdin, sys.stdout = orig_stdin, orig_stdout
                self.assertEqual(stdout_3.getvalue().strip(), "")
            finally:
                guard.RENAME_STATE_DIR = orig_dir

    def test_concurrent_multi_file_rename_serialization(self) -> None:
        import concurrent.futures
        with tempfile.TemporaryDirectory() as tmpdir:
            orig_dir = guard.RENAME_STATE_DIR
            try:
                guard.RENAME_STATE_DIR = tmpdir
                conv = "test-concurrent-conv"

                def rename_worker(file_path: str):
                    return guard.check_multi_file_rename(
                        conv_id=conv,
                        file_path=file_path,
                        old_sym="shared_sym",
                        new_sym="renamed_sym",
                        repo_root="/repo",
                    )

                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
                    f1 = executor.submit(rename_worker, "/repo/src/first.rs")
                    f2 = executor.submit(rename_worker, "/repo/src/second.rs")
                    res1 = f1.result()
                    res2 = f2.result()

                results = [res1[0], res2[0]]
                self.assertEqual(sorted(results), [False, True], f"Concurrent results: {res1} and {res2}")
            finally:
                guard.RENAME_STATE_DIR = orig_dir

    def test_codex_apply_patch_parsing_and_detection(self) -> None:
        patch_text = (
            "*** Begin Patch\n"
            "*** Update File: /repo/src/lib.rs\n"
            "@@ -10,3 +10,3 @@\n"
            "-let x = old_sym;\n"
            "+let x = new_sym;\n"
            "*** Update File: /repo/src/main.rs\n"
            "@@ -5,2 +5,2 @@\n"
            "-use crate::old_sym;\n"
            "+use crate::new_sym;\n"
            "*** End Patch"
        )
        payload = {
            "tool_name": "apply_patch",
            "tool_input": {
                "cmd": patch_text,
            },
        }
        edits = guard.file_edits_from_payload(payload)
        self.assertEqual(len(edits), 2)
        self.assertEqual(edits[0][0], "/repo/src/lib.rs")
        self.assertEqual(guard.single_symbol_rename(edits[0][1], edits[0][2]), ("old_sym", "new_sym"))
        self.assertEqual(edits[1][0], "/repo/src/main.rs")
        self.assertEqual(guard.single_symbol_rename(edits[1][1], edits[1][2]), ("old_sym", "new_sym"))
        self.assertFalse(edits[0][3])

        # Test override inside patch
        patch_override = patch_text + "\n# PROD_CODE_MANUAL_RENAME=1"
        payload_override = {"tool_name": "apply_patch", "tool_input": {"cmd": patch_override}}
        edits_override = guard.file_edits_from_payload(payload_override)
        self.assertTrue(all(e[3] for e in edits_override))

    def test_codex_apply_patch_multi_file_denied_in_main(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            orig_dir = guard.RENAME_STATE_DIR
            repo_root = str(Path(__file__).parent.parent)
            file_a = os.path.join(repo_root, "scripts/hooks/file_a.py")
            file_b = os.path.join(repo_root, "scripts/hooks/file_b.py")
            try:
                guard.RENAME_STATE_DIR = tmpdir
                # Single patch touching 2 files with the same symbol rename
                multi_patch = (
                    "*** Begin Patch\n"
                    f"*** Update File: {file_a}\n"
                    "@@ -1,1 +1,1 @@\n"
                    "-my_symbol = 1\n"
                    "+new_symbol = 1\n"
                    f"*** Update File: {file_b}\n"
                    "@@ -1,1 +1,1 @@\n"
                    "-my_symbol = 2\n"
                    "+new_symbol = 2\n"
                    "*** End Patch"
                )
                payload = {
                    "tool_name": "apply_patch",
                    "tool_input": {"cmd": multi_patch},
                    "cwd": repo_root,
                    "session_id": "test-codex-session",
                }
                orig_stdin, orig_stdout = sys.stdin, sys.stdout
                stdout = io.StringIO()
                try:
                    sys.stdin = io.StringIO(json.dumps(payload))
                    sys.stdout = stdout
                    guard.main()
                finally:
                    sys.stdin, sys.stdout = orig_stdin, orig_stdout

                out = stdout.getvalue().strip()
                self.assertTrue(out)
                res = json.loads(out)
                self.assertIn("hookSpecificOutput", res)
                self.assertEqual(res["hookSpecificOutput"]["permissionDecision"], "deny")
                self.assertIn("Multi-file symbol rename detected", res["hookSpecificOutput"]["permissionDecisionReason"])
                self.assertIn("code_rename", res["hookSpecificOutput"]["permissionDecisionReason"])
            finally:
                guard.RENAME_STATE_DIR = orig_dir

    def test_claude_edit_multi_file_denied_in_main(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            orig_dir = guard.RENAME_STATE_DIR
            repo_root = str(Path(__file__).parent.parent)
            file_a = os.path.join(repo_root, "scripts/hooks/file_a.py")
            file_b = os.path.join(repo_root, "scripts/hooks/file_b.py")
            try:
                guard.RENAME_STATE_DIR = tmpdir
                session = "test-claude-session"

                # 1st Edit: allowed
                payload_1 = {
                    "tool_name": "Edit",
                    "tool_input": {
                        "file_path": file_a,
                        "old_string": "def old_foo():",
                        "new_string": "def new_foo():",
                    },
                    "cwd": repo_root,
                    "session_id": session,
                }
                orig_stdin, orig_stdout = sys.stdin, sys.stdout
                stdout_1 = io.StringIO()
                try:
                    sys.stdin = io.StringIO(json.dumps(payload_1))
                    sys.stdout = stdout_1
                    guard.main()
                finally:
                    sys.stdin, sys.stdout = orig_stdin, orig_stdout
                self.assertEqual(stdout_1.getvalue().strip(), "")

                # 2nd Edit on different file with same rename: denied with hookSpecificOutput
                payload_2 = {
                    "tool_name": "Edit",
                    "tool_input": {
                        "file_path": file_b,
                        "old_string": "old_foo()",
                        "new_string": "new_foo()",
                    },
                    "cwd": repo_root,
                    "session_id": session,
                }
                stdout_2 = io.StringIO()
                try:
                    sys.stdin = io.StringIO(json.dumps(payload_2))
                    sys.stdout = stdout_2
                    guard.main()
                finally:
                    sys.stdin, sys.stdout = orig_stdin, orig_stdout

                out_2 = stdout_2.getvalue().strip()
                self.assertTrue(out_2)
                res = json.loads(out_2)
                self.assertEqual(res["hookSpecificOutput"]["permissionDecision"], "deny")
                self.assertIn("Multi-file symbol rename detected", res["hookSpecificOutput"]["permissionDecisionReason"])
                self.assertIn("code_rename", res["hookSpecificOutput"]["permissionDecisionReason"])
            finally:
                guard.RENAME_STATE_DIR = orig_dir


class ProdCodeGuardSymbolGrepPolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp_dir.cleanup)
        self.transcript_file = os.path.join(self.temp_dir.name, "transcript.jsonl")

    def test_reported_prod_code_issue_check(self) -> None:
        # Without transcript file -> True (human interactive fallback)
        self.assertTrue(guard.reported_prod_code_issue(None, "my_func"))
        self.assertTrue(guard.reported_prod_code_issue("/nonexistent/path", "my_func"))

        # Empty transcript -> False
        with open(self.transcript_file, "w") as f:
            f.write(json.dumps({"step": 1, "type": "USER_INPUT"}) + "\n")
        self.assertFalse(guard.reported_prod_code_issue(self.transcript_file, "my_func"))

        # Transcript with code_report_issue -> True
        with open(self.transcript_file, "a") as f:
            f.write(json.dumps({"step": 2, "tool_call": "prod-code__code_report_issue", "args": {"title": "missing my_func"}}) + "\n")
        self.assertTrue(guard.reported_prod_code_issue(self.transcript_file, "my_func"))

    def test_symbol_grep_denial_message_never_mentions_override_cheat(self) -> None:
        repo_root = str(Path(__file__).parent.parent)
        payload = {
            "toolCall": {
                "name": "run_command",
                "args": {
                    "CommandLine": 'git grep -n "my_symbol" scripts/',
                    "Cwd": repo_root,
                },
            },
            "workspacePaths": [repo_root],
            "transcript_path": self.transcript_file,
        }
        with open(self.transcript_file, "w") as f:
            f.write("{}\n")

        stdout = io.StringIO()
        orig_stdin, orig_stdout = sys.stdin, sys.stdout
        try:
            sys.stdin = io.StringIO(json.dumps(payload))
            sys.stdout = stdout
            guard.main()
        finally:
            sys.stdin, sys.stdout = orig_stdin, orig_stdout

        out = stdout.getvalue().strip()
        self.assertTrue(out)
        res = json.loads(out)
        self.assertEqual(res.get("decision"), "deny")
        reason = res.get("reason", "")
        self.assertIn("Grep for code symbol `my_symbol` is blocked", reason)
        self.assertIn("code_definition", reason)
        self.assertIn("code_report_issue", reason)
        # CRITICAL: MUST NOT mention PROD_CODE_GREP
        self.assertNotIn("PROD_CODE_GREP", reason)

    def test_override_requires_both_query_and_issue_report(self) -> None:
        repo_root = str(Path(__file__).parent.parent)
        override_cmd = 'PROD_CODE_GREP=1 git grep -n "my_symbol" scripts/'

        # Case 1: Neither asked prod-code nor reported issue
        with open(self.transcript_file, "w") as f:
            f.write("{}\n")

        payload = {
            "toolCall": {
                "name": "run_command",
                "args": {"CommandLine": override_cmd, "Cwd": repo_root},
            },
            "workspacePaths": [repo_root],
            "transcript_path": self.transcript_file,
        }

        stdout = io.StringIO()
        orig_stdin, orig_stdout = sys.stdin, sys.stdout
        try:
            sys.stdin = io.StringIO(json.dumps(payload))
            sys.stdout = stdout
            guard.main()
        finally:
            sys.stdin, sys.stdout = orig_stdin, orig_stdout

        res = json.loads(stdout.getvalue().strip())
        self.assertEqual(res.get("decision"), "deny")
        self.assertIn("Ask prod-code first", res.get("reason", ""))
        self.assertNotIn("PROD_CODE_GREP", res.get("reason", ""))

        # Case 2: Asked prod-code, but did NOT report issue
        with open(self.transcript_file, "w") as f:
            f.write(json.dumps({"tool": "code_definition", "args": {"symbol": "my_symbol"}}) + "\n")

        stdout_2 = io.StringIO()
        try:
            sys.stdin = io.StringIO(json.dumps(payload))
            sys.stdout = stdout_2
            guard.main()
        finally:
            sys.stdin, sys.stdout = orig_stdin, orig_stdout

        res_2 = json.loads(stdout_2.getvalue().strip())
        self.assertEqual(res_2.get("decision"), "deny")
        self.assertIn("report the bug first: code_report_issue", res_2.get("reason", ""))
        self.assertNotIn("PROD_CODE_GREP", res_2.get("reason", ""))

        # Case 3: Asked prod-code AND reported issue -> override allowed!
        with open(self.transcript_file, "a") as f:
            f.write(json.dumps({"tool": "code_report_issue", "args": {"title": "Symbol not found"}}) + "\n")

        stdout_3 = io.StringIO()
        try:
            sys.stdin = io.StringIO(json.dumps(payload))
            sys.stdout = stdout_3
            guard.main()
        finally:
            sys.stdin, sys.stdout = orig_stdin, orig_stdout

        # Antigravity does not print decision on allow
        self.assertEqual(stdout_3.getvalue().strip(), "")

    def test_agent_without_transcript_is_blocked(self) -> None:
        # For AI agents, lack of transcript must block the bypass rather than assuming True
        self.assertFalse(guard.reported_prod_code_issue(None, "my_func", agent="Bash"))
        self.assertFalse(guard.reported_prod_code_issue("/nonexistent/path", "my_func", agent="Bash"))
        self.assertFalse(guard.asked_prod_code(None, "my_func", agent="Bash"))
        self.assertFalse(guard.asked_prod_code("/nonexistent/path", "my_func", agent="Bash"))

    def test_claude_code_session_transcript_resolution(self) -> None:
        repo_root = str(Path(__file__).parent.parent)
        session_id = "test-claude-session-1234"
        cwd_slug = repo_root.replace("/", "-")
        fake_home = self.temp_dir.name
        claude_dir = os.path.join(fake_home, ".claude", "projects", f"-{cwd_slug.lstrip('-')}")
        os.makedirs(claude_dir, exist_ok=True)
        transcript_path = os.path.join(claude_dir, f"{session_id}.jsonl")

        with open(transcript_path, "w") as f:
            f.write(json.dumps({"type": "tool_use", "name": "mcp__prod-code__code_definition", "input": {"symbol": "target_symbol"}}) + "\n")

        payload = {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {
                "command": 'PROD_CODE_GREP=1 git grep -n "target_symbol" scripts/',
            },
            "session_id": session_id,
            "cwd": repo_root,
        }

        # Case 1: Asked prod-code in transcript, but did not report issue yet -> denied
        with mock.patch("os.path.expanduser", side_effect=lambda p: p.replace("~", fake_home)):
            stdout = io.StringIO()
            orig_stdin, orig_stdout = sys.stdin, sys.stdout
            try:
                sys.stdin = io.StringIO(json.dumps(payload))
                sys.stdout = stdout
                guard.main()
            finally:
                sys.stdin, sys.stdout = orig_stdin, orig_stdout

            out = stdout.getvalue().strip()
            self.assertTrue(out)
            res = json.loads(out)
            self.assertEqual(res["hookSpecificOutput"]["permissionDecision"], "deny")
            self.assertIn("report the bug first: code_report_issue", res["hookSpecificOutput"]["permissionDecisionReason"])

            # Case 2: Now append code_report_issue to transcript -> allowed
            with open(transcript_path, "a") as f:
                f.write(json.dumps({"type": "tool_use", "name": "mcp__prod-code__code_report_issue", "input": {"title": "Symbol not found: target_symbol"}}) + "\n")

            stdout_2 = io.StringIO()
            try:
                sys.stdin = io.StringIO(json.dumps(payload))
                sys.stdout = stdout_2
                guard.main()
            finally:
                sys.stdin, sys.stdout = orig_stdin, orig_stdout

            # Allowed -> no denial output printed for Bash
            self.assertEqual(stdout_2.getvalue().strip(), "")


if __name__ == "__main__":
    unittest.main()
