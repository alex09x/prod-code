# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Symbol-grep override policy, transcript verification, and text-grep routing test cases."""

from __future__ import annotations

import json
import os
import tempfile
from unittest import mock

from .support import GuardTestCaseBase, make_git_repo_root


class SymbolGrepPolicyCasesMixin(GuardTestCaseBase):
    temp_dir: tempfile.TemporaryDirectory[str] | None = None
    transcript_file: str = ""
    repo_root: str = ""

    def setUp(self) -> None:
        temp_dir = tempfile.TemporaryDirectory()
        self.temp_dir = temp_dir
        self.addCleanup(temp_dir.cleanup)
        self.transcript_file = os.path.join(temp_dir.name, "transcript.jsonl")
        self.repo_root = str(make_git_repo_root(self))

    def test_reported_prod_code_issue_check(self) -> None:
        guard = self._guard()
        # Without transcript file -> True (human interactive fallback)
        self.assertTrue(guard.reported_prod_code_issue(None, "my_func"))
        self.assertTrue(guard.reported_prod_code_issue("/nonexistent/path", "my_func"))

        # Empty transcript -> False
        with open(self.transcript_file, "w") as f:
            f.write(json.dumps({"step": 1, "type": "USER_INPUT"}) + "\n")
        self.assertFalse(guard.reported_prod_code_issue(self.transcript_file, "my_func"))

        # Transcript with code_report_issue -> True
        with open(self.transcript_file, "a") as f:
            f.write(
                json.dumps(
                    {
                        "step": 2,
                        "tool_call": "prod-code__code_report_issue",
                        "args": {"title": "missing my_func"},
                    }
                )
                + "\n"
            )
        self.assertTrue(guard.reported_prod_code_issue(self.transcript_file, "my_func"))

    def test_symbol_grep_denial_message_never_mentions_override_cheat(self) -> None:
        repo_root = self.repo_root
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

        out = self._invoke_main_stdio(payload)
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
        repo_root = self.repo_root
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

        res = json.loads(self._invoke_main_stdio(payload))
        self.assertEqual(res.get("decision"), "deny")
        self.assertIn("Ask prod-code first", res.get("reason", ""))
        self.assertNotIn("PROD_CODE_GREP", res.get("reason", ""))

        # Case 2: Asked prod-code, but did NOT report issue
        with open(self.transcript_file, "w") as f:
            f.write(json.dumps({"tool": "code_definition", "args": {"symbol": "my_symbol"}}) + "\n")

        res_2 = json.loads(self._invoke_main_stdio(payload))
        self.assertEqual(res_2.get("decision"), "deny")
        self.assertIn("report the bug first: code_report_issue", res_2.get("reason", ""))
        self.assertNotIn("PROD_CODE_GREP", res_2.get("reason", ""))

        # Case 3: Asked prod-code AND reported issue -> override allowed!
        with open(self.transcript_file, "a") as f:
            f.write(
                json.dumps({"tool": "code_report_issue", "args": {"title": "Symbol not found"}})
                + "\n"
            )

        # Antigravity does not print decision on allow
        self.assertEqual(self._invoke_main_stdio(payload), "")

    def test_agent_without_transcript_is_blocked(self) -> None:
        guard = self._guard()
        # For AI agents, lack of transcript must block the bypass rather than assuming True
        self.assertFalse(guard.reported_prod_code_issue(None, "my_func", agent="Bash"))
        self.assertFalse(guard.reported_prod_code_issue("/nonexistent/path", "my_func", agent="Bash"))
        self.assertFalse(guard.asked_prod_code(None, "my_func", agent="Bash"))
        self.assertFalse(guard.asked_prod_code("/nonexistent/path", "my_func", agent="Bash"))

    def test_claude_code_session_transcript_resolution(self) -> None:
        repo_root = self.repo_root
        session_id = "test-claude-session-1234"
        cwd_slug = repo_root.replace("/", "-")
        assert self.temp_dir is not None
        fake_home = self.temp_dir.name
        claude_dir = os.path.join(fake_home, ".claude", "projects", f"-{cwd_slug.lstrip('-')}")
        os.makedirs(claude_dir, exist_ok=True)
        transcript_path = os.path.join(claude_dir, f"{session_id}.jsonl")

        with open(transcript_path, "w") as f:
            f.write(
                json.dumps(
                    {
                        "type": "tool_use",
                        "name": "mcp__prod-code__code_definition",
                        "input": {"symbol": "target_symbol"},
                    }
                )
                + "\n"
            )

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
            out = self._invoke_main_stdio(payload)
            self.assertTrue(out)
            res = json.loads(out)
            self.assertEqual(res["hookSpecificOutput"]["permissionDecision"], "deny")
            self.assertIn(
                "report the bug first: code_report_issue",
                res["hookSpecificOutput"]["permissionDecisionReason"],
            )

            # Case 2: Now append code_report_issue to transcript -> allowed
            with open(transcript_path, "a") as f:
                f.write(
                    json.dumps(
                        {
                            "type": "tool_use",
                            "name": "mcp__prod-code__code_report_issue",
                            "input": {"title": "Symbol not found: target_symbol"},
                        }
                    )
                    + "\n"
                )

            # Allowed -> no denial output printed for Bash
            self.assertEqual(self._invoke_main_stdio(payload), "")

    def test_heredoc_and_inline_script_edit_verification_allowed_issue_1125(self) -> None:
        guard = self._guard()
        repo_root = self.repo_root
        cmd_example_4 = (
            "python3 - <<'EOF'\n"
            "p='construction_handoff.go'\n"
            "s=open(p).read()\n"
            's=s.replace("acceptCtx, cancel := context.WithTimeout(parent, 3*time.Second)","acceptCtx, cancel := context.WithTimeout(parent, constructionAcceptTimeout)",1)\n'
            's=s.replace("const constructionAcceptRetries = 2\\n","const constructionAcceptRetries = 2\\n\\nvar constructionAcceptTimeout = 3 * time.Second\\n",1)\n'
            "open(p,'w').write(s)\n"
            "EOF\n"
            "grep -c constructionAcceptTimeout construction_handoff.go"
        )
        self.assertIsNone(guard.symbol_grep(cmd_example_4, repo_root))
        self.assertIsNone(guard.text_grep(cmd_example_4, repo_root))

        cmd_escaped_eof = (
            "python3 - <<\\EOF\n"
            "s = '`grep -rn someIdentifier src/`'\n"
            "EOF\n"
        )
        self.assertIsNone(guard.symbol_grep(cmd_escaped_eof, repo_root))
        self.assertIsNone(guard.text_grep(cmd_escaped_eof, repo_root))

        cmd_unquoted_python = (
            "python3 - <<EOF\n"
            "doc = '`rg my_symbol crates/`'\n"
            "EOF\n"
        )
        self.assertIsNone(guard.symbol_grep(cmd_unquoted_python, repo_root))
        self.assertIsNone(guard.text_grep(cmd_unquoted_python, repo_root))

    def test_text_grep_routes_non_symbol_patterns_to_code_grep_issue_1119(self) -> None:
        guard = self._guard()
        repo_root = self.repo_root
        lib = "crates/prod-code-mcp/src/lib.rs"
        # (command, pattern as grep read it, path, the same pattern for `prod-code grep`). A basic
        # grep pattern is converted to ripgrep's syntax: `\|` alternates there only as `|` (#1581).
        cases = [
            ('rg "fn test_" crates/', "fn test_", "crates/", "fn test_"),
            ('rg "reserve_" crates/', "reserve_", "crates/", "reserve_"),
            (f"grep -n 'ctx\\|cancel' {lib}", "ctx\\|cancel", lib, "ctx|cancel"),
            ('rg "TODO|FIXME" crates/', "TODO|FIXME", "crates/", "TODO|FIXME"),
            (f"grep -En 'TODO|FIXME' {lib}", "TODO|FIXME", lib, "TODO|FIXME"),
            (f"grep -n 'wait (ms)' {lib}", "wait (ms)", lib, "wait \\(ms\\)"),
            (f"grep -n 'x\\{{2\\}}[|]' {lib}", "x\\{2\\}[|]", lib, "x{2}[|]"),
            (f"grep -nF 'a+b' {lib}", "a+b", lib, "a\\+b"),
        ]
        for cmd, expected_pat, expected_path, suggested in cases:
            with self.subTest(command=cmd):
                self.assertIsNone(guard.symbol_grep(cmd, repo_root))
                res = guard.text_grep(cmd, repo_root)
                self.assertEqual(res, (expected_pat, expected_path))

                payload = {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Bash",
                    "tool_input": {"command": cmd},
                    "cwd": repo_root,
                }
                out = json.loads(self._invoke_main_stdio(payload))
                reason = out["hookSpecificOutput"]["permissionDecisionReason"]
                self.assertIn(f'code_grep {{pattern: "{suggested}", path: "{expected_path}"}}', reason)
                self.assertIn(f'prod-code grep "{suggested}" {expected_path}', reason)
