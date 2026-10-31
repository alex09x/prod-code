# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""End-to-end hook entrypoint test cases for multi-file symbol rename interception."""

from __future__ import annotations

import json
import os
from pathlib import Path

from .support import GuardTestCaseBase, make_git_repo_root


class MultiFileRenameHookCasesMixin(GuardTestCaseBase):
    repo_root: Path = Path()

    def setUp(self) -> None:
        self.repo_root = make_git_repo_root(self)

    def test_end_to_end_antigravity_rename_guard(self) -> None:
        with self._temporary_rename_state_dir():
            conv = "conv-e2e"

            # Edit 1 on file a.rs (inside git repo)
            payload_1 = {
                "toolCall": {
                    "name": "replace_file_content",
                    "args": {
                        "TargetFile": str(
                            self.repo_root / "scripts" / "hooks" / "file_a.py"
                        ),
                        "TargetContent": "my_symbol",
                        "ReplacementContent": "my_new_symbol",
                        "Instruction": "rename",
                        "Description": "rename",
                    },
                },
                "conversationId": conv,
                "workspacePaths": [str(self.repo_root)],
            }
            self.assertEqual(self._invoke_main_stdio(payload_1), "")

            # Edit 2 on file b.py with SAME symbol -> should be DENIED
            payload_2 = {
                "toolCall": {
                    "name": "replace_file_content",
                    "args": {
                        "TargetFile": str(
                            self.repo_root / "scripts" / "hooks" / "file_b.py"
                        ),
                        "TargetContent": "my_symbol",
                        "ReplacementContent": "my_new_symbol",
                        "Instruction": "rename",
                        "Description": "rename",
                    },
                },
                "conversationId": conv,
                "workspacePaths": [str(self.repo_root)],
            }
            out_json = json.loads(self._invoke_main_stdio(payload_2))
            self.assertEqual(out_json.get("decision"), "deny")
            self.assertIn("code_rename", out_json.get("reason", ""))
            self.assertIn("file_a.py", out_json.get("reason", ""))

            # Edit 3 on file c.py with SAME symbol but PROD_CODE_MANUAL_RENAME=1 override -> ALLOWED
            payload_3 = {
                "toolCall": {
                    "name": "replace_file_content",
                    "args": {
                        "TargetFile": str(
                            self.repo_root / "scripts" / "hooks" / "file_c.py"
                        ),
                        "TargetContent": "my_symbol",
                        "ReplacementContent": "my_new_symbol",
                        "Instruction": "PROD_CODE_MANUAL_RENAME=1",
                        "Description": "manual rename override",
                    },
                },
                "conversationId": conv,
                "workspacePaths": [str(self.repo_root)],
            }
            self.assertEqual(self._invoke_main_stdio(payload_3), "")

    def test_codex_apply_patch_multi_file_denied_in_main(self) -> None:
        repo_root = str(self.repo_root)
        file_a = os.path.join(repo_root, "scripts/hooks/file_a.py")
        file_b = os.path.join(repo_root, "scripts/hooks/file_b.py")
        with self._temporary_rename_state_dir():
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
            out = self._invoke_main_stdio(payload)
            self.assertTrue(out)
            res = json.loads(out)
            self.assertIn("hookSpecificOutput", res)
            self.assertEqual(res["hookSpecificOutput"]["permissionDecision"], "deny")
            self.assertIn(
                "Multi-file symbol rename detected",
                res["hookSpecificOutput"]["permissionDecisionReason"],
            )
            self.assertIn(
                "code_rename", res["hookSpecificOutput"]["permissionDecisionReason"]
            )

    def test_claude_edit_multi_file_denied_in_main(self) -> None:
        repo_root = str(self.repo_root)
        file_a = os.path.join(repo_root, "scripts/hooks/file_a.py")
        file_b = os.path.join(repo_root, "scripts/hooks/file_b.py")
        with self._temporary_rename_state_dir():
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
            self.assertEqual(self._invoke_main_stdio(payload_1), "")

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
            out_2 = self._invoke_main_stdio(payload_2)
            self.assertTrue(out_2)
            res = json.loads(out_2)
            self.assertEqual(res["hookSpecificOutput"]["permissionDecision"], "deny")
            self.assertIn(
                "Multi-file symbol rename detected",
                res["hookSpecificOutput"]["permissionDecisionReason"],
            )
            self.assertIn(
                "code_rename", res["hookSpecificOutput"]["permissionDecisionReason"]
            )
