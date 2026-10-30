# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Unit, state-file, concurrency, and edit-payload extraction cases for multi-file renames."""

from __future__ import annotations

import concurrent.futures
import os
import time
from unittest import mock

from .support import GuardTestCaseBase


class MultiFileRenameUnitCasesMixin(GuardTestCaseBase):
    def test_single_symbol_rename_detection(self) -> None:
        guard = self._guard()
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
        for kw in (
            "fn",
            "func",
            "let",
            "mut",
            "var",
            "const",
            "class",
            "struct",
            "type",
            "return",
            "if",
            "switch",
        ):
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
        self.assertIsNone(
            guard.single_symbol_rename("if x > 0 { return true; }", "if x >= 0 { return false; }")
        )
        self.assertIsNone(
            guard.single_symbol_rename("let a = 1; let b = 2;", "let c = 1; let d = 2;")
        )
        self.assertEqual(guard.single_symbol_rename("fn old() {}", "fn new() {}"), ("old", "new"))
        self.assertIsNone(guard.single_symbol_rename("fn old() { 1 }", "fn old() { 2 }"))
        # Identical or empty
        self.assertIsNone(guard.single_symbol_rename("foo", "foo"))
        self.assertIsNone(guard.single_symbol_rename("", "bar"))
        self.assertIsNone(guard.single_symbol_rename("foo", ""))

    def test_multi_file_rename_state_tracking(self) -> None:
        guard = self._guard()
        with self._temporary_rename_state_dir() as tmpdir:
            conv = "test-conv-123"

            # 1st file: allowed
            denied, other = guard.check_multi_file_rename(
                conv, "/repo/src/a.rs", "old_func", "new_func", repo_root="/repo"
            )
            self.assertFalse(denied)
            self.assertIsNone(other)

            # Verify file permissions mode 0600
            state_file = os.path.join(tmpdir, f"rename_{conv}_repo.json")
            self.assertTrue(os.path.exists(state_file))
            file_mode = os.stat(state_file).st_mode & 0o777
            self.assertEqual(file_mode, 0o600)

            # Same file again: allowed
            denied, other = guard.check_multi_file_rename(
                conv, "/repo/src/a.rs", "old_func", "new_func", repo_root="/repo"
            )
            self.assertFalse(denied)
            self.assertIsNone(other)

            # Different rename target for same old symbol (config -> cfg vs config -> settings): allowed!
            denied, other = guard.check_multi_file_rename(
                conv, "/repo/src/b.rs", "old_func", "distinct_target", repo_root="/repo"
            )
            self.assertFalse(denied)
            self.assertIsNone(other)

            # Different repository for same (old_sym, new_sym): allowed!
            denied, other = guard.check_multi_file_rename(
                conv, "/other_repo/src/b.rs", "old_func", "new_func", repo_root="/other_repo"
            )
            self.assertFalse(denied)
            self.assertIsNone(other)

            # Different file with same (old_sym, new_sym) in SAME repo: denied!
            denied, other = guard.check_multi_file_rename(
                conv, "/repo/src/b.rs", "old_func", "new_func", repo_root="/repo"
            )
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
            self.assertTrue(
                os.path.exists(stale_lock),
                "Lock files must never be age-unlinked to prevent split inode races",
            )

            # Lock acquisition failure fails safely (returns False, None) without exception
            with mock.patch("fcntl.flock", side_effect=OSError("flock failed")):
                denied, other = guard.check_multi_file_rename(
                    conv, "/repo/src/d.rs", "foo", "bar", repo_root="/repo"
                )
                self.assertFalse(denied)
                self.assertIsNone(other)

    def test_file_edit_details_extraction(self) -> None:
        guard = self._guard()
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

    def test_concurrent_multi_file_rename_serialization(self) -> None:
        guard = self._guard()
        with self._temporary_rename_state_dir():
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

    def test_codex_apply_patch_parsing_and_detection(self) -> None:
        guard = self._guard()
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
