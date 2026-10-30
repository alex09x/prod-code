# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Inline Python source-file rewrite detection and hook denial test cases."""

from __future__ import annotations

import io
import json
import os
import tempfile
from collections.abc import Mapping
from pathlib import Path
from unittest import mock

from .support import GuardTestCaseBase


class ScriptedSourceWriteFixtureBase(GuardTestCaseBase):
    repo_dir: tempfile.TemporaryDirectory[str] | None = None
    cwd: str = ""

    def setUp(self) -> None:
        repo_dir = tempfile.TemporaryDirectory()
        self.repo_dir = repo_dir
        self.addCleanup(repo_dir.cleanup)
        self.cwd = repo_dir.name
        for rel in (
            ".git",
            "crates/prod-code-mcp/src",
            "src",
            "scripts",
            "docs",
            "target",
        ):
            os.makedirs(os.path.join(self.cwd, rel), exist_ok=True)

    def _run_hook(self, payload: Mapping[str, object]) -> tuple[str, mock.MagicMock]:
        guard = self._guard()
        stdout = io.StringIO()
        with (
            mock.patch.object(guard, "log") as log_mock,
            mock.patch("sys.stdin", io.StringIO(json.dumps(payload))),
            mock.patch("sys.stdout", stdout),
        ):
            rc = guard.main()
        self.assertEqual(rc, 0)
        return stdout.getvalue().strip(), log_mock


class ScriptedSourceWriteDenialCasesMixin(ScriptedSourceWriteFixtureBase):
    def test_issue_1760_claude_pretool_use_variable_bound_rewrite_denied(self) -> None:
        command = (
            'python3 -c \'p="crates/prod-code-mcp/src/lib.rs"; '
            's=open(p).read(); open(p,"w").write(s.replace("before","after"))\''
        )
        payload = {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "cwd": self.cwd,
        }
        raw_out, log_mock = self._run_hook(payload)
        result = json.loads(raw_out)
        output = result["hookSpecificOutput"]
        self.assertEqual(output["permissionDecision"], "deny")
        reason = output["permissionDecisionReason"]
        self.assertIn("crates/prod-code-mcp/src/lib.rs", reason)
        self.assertIn("Edit", reason)
        self.assertIn("apply_patch", reason)
        log_mock.assert_called_once()
        entry = log_mock.call_args.args[0]
        self.assertEqual(entry["rule"], "scripted-source-write")
        self.assertEqual(entry["decision"], "deny")
        self.assertEqual(entry["file"], "crates/prod-code-mcp/src/lib.rs")

    def test_codex_antigravity_and_gemini_payloads_deny_with_edit_guidance(
        self,
    ) -> None:
        cmd = 'python3 -c \'from pathlib import Path; Path("src/main.rs").write_text("fn main() {}\\n")\''

        codex_payload = {
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": ["bash", "-lc", cmd]},
            "cwd": self.cwd,
        }
        raw_codex, _ = self._run_hook(codex_payload)
        codex_res = json.loads(raw_codex)["hookSpecificOutput"]
        self.assertEqual(codex_res["permissionDecision"], "deny")
        self.assertIn("src/main.rs", codex_res["permissionDecisionReason"])

        agy_payload = {
            "toolCall": {"name": "run_command", "args": {"CommandLine": cmd}},
            "workspacePaths": [self.cwd],
        }
        raw_agy, _ = self._run_hook(agy_payload)
        agy_res = json.loads(raw_agy)
        self.assertEqual(agy_res["decision"], "deny")
        self.assertIn("Edit", agy_res["reason"])
        self.assertIn("replace_file_content", agy_res["reason"])
        self.assertIn("apply_patch", agy_res["reason"])

        gemini_payload = {
            "hook_event_name": "BeforeTool",
            "tool_name": "run_shell_command",
            "tool_input": {"command": cmd},
            "cwd": self.cwd,
        }
        raw_gem, _ = self._run_hook(gemini_payload)
        gem_res = json.loads(raw_gem)
        self.assertEqual(gem_res["decision"], "deny")
        self.assertIn("src/main.rs", gem_res["reason"])

    def test_inline_python_c_and_heredoc_source_writes_detected(self) -> None:
        guard = self._guard()
        commands = [
            'python3 -c \'open("crates/prod-code-mcp/src/lib.rs", "w").write("x")\'',
            'python -c \'with open("src/main.go", mode="w+", encoding="utf-8") as f: f.write("package main\\n")\'',
            'python3.12 -uc \'from pathlib import Path; p = Path("crates") / "app" / "main.ts"; p.write_text("export {}\\n")\'',
            'python3 -c \'import os; p = os.path.join("src", "handler.py"); open(p, "a").write("# x\\n")\'',
            'python3 -c \'crate = "prod-code-mcp"; p = f"crates/{crate}/src/lib.rs"; open(p, "w").write("x")\'',
            'python3 -c \'import sys; p = sys.argv[1]; open(p, "w").write("x")\' crates/prod-code-mcp/src/lib.rs',
            'python3 -c \'import os; tmp = "src/lib.rs.tmp"; open(tmp, "w").write("x"); os.replace(tmp, "src/lib.rs")\'',
            'python3 -c \'from pathlib import Path; [Path(p).write_text("x") for p in ["src/a.rs", "src/b.rs"]]\'',
            "python3 -c $'p = \\'src/lib.rs\\'\\nopen(p, \\'w\\').write(\\'x\\')'",
            'env LANG=C bash -lc \'python3 -c "open(\\"src/lib.rs\\", \\"w\\").write(\\"x\\")"\'',
            (
                "python3 - <<'EOF'\n"
                "p = 'crates/prod-code-mcp/src/lib.rs'\n"
                "s = open(p).read()\n"
                "open(p, 'w').write(s.replace('a', 'b'))\n"
                "EOF"
            ),
            (
                "python3 <<EOF 2>/dev/null\n"
                "from pathlib import Path\n"
                "Path('src/widget.tsx').write_bytes(b'export const x = 1;\\n')\n"
                "EOF"
            ),
            (
                "python3 <<-'EOF'\n"
                "\tp = 'src/server.go'\n"
                "\topen(p, 'w').write('package main\\n')\n"
                "\tEOF"
            ),
            (
                "cat <<'EOF' | python3 -\n"
                "open('src/lib.rs', 'w').write('// edited\\n')\n"
                "EOF"
            ),
            'python3 <<< \'open("src/lib.rs", "w").write("x")\'',
        ]
        for command in commands:
            with self.subTest(command=command):
                self.assertIsNotNone(
                    guard.scripted_source_write_target(command, self.cwd)
                )

    def test_local_helper_function_called_with_literal_or_bound_source_path_detected(
        self,
    ) -> None:
        guard = self._guard()
        heredoc_helper = (
            "python3 - <<'EOF'\n"
            "def patch(path, old, new):\n"
            "    text = open(path).read()\n"
            "    assert old in text\n"
            "    open(path, 'w').write(text.replace(old, new, 1))\n"
            "\n"
            "patch('crates/prod-code-mcp/src/lib.rs', 'before', 'after')\n"
            "EOF"
        )
        self.assertEqual(
            guard.scripted_source_write_target(heredoc_helper, self.cwd),
            "crates/prod-code-mcp/src/lib.rs",
        )

        kw_and_default_helpers = [
            (
                "python3 - <<'EOF'\n"
                "from pathlib import Path\n"
                "def update_file(target):\n"
                "    p = Path(target)\n"
                "    p.write_text(p.read_text().strip() + '\\n')\n"
                "update_file(target='src/main.rs')\n"
                "EOF"
            ),
            (
                "python3 - <<'EOF'\n"
                "from pathlib import Path\n"
                "def rewrite(p='src/default_target.py'):\n"
                "    with Path(p).open('w') as fh:\n"
                "        fh.write('x = 1\\n')\n"
                "if __name__ == '__main__':\n"
                "    rewrite()\n"
                "EOF"
            ),
            (
                "python3 - <<'EOF'\n"
                "from pathlib import Path\n"
                "def crate_src(rel):\n"
                "    return Path('crates/prod-code-mcp') / rel\n"
                "crate_src('src/lib.rs').write_text('// updated\\n')\n"
                "EOF"
            ),
        ]
        for command in kw_and_default_helpers:
            with self.subTest(command=command):
                self.assertIsNotNone(
                    guard.scripted_source_write_target(command, self.cwd)
                )

    def test_direct_shell_cd_and_tmp_git_worktree_are_enforced(self) -> None:
        guard = self._guard()
        tmp_base = "/tmp" if os.path.isdir("/tmp") else None
        with (
            tempfile.TemporaryDirectory(dir=tmp_base) as wt_root,
            tempfile.TemporaryDirectory(dir=tmp_base) as outside_dir,
        ):
            # Real Git worktrees carry a `.git` file pointing at the common repo's worktrees entry.
            (Path(wt_root) / ".git").write_text(
                "gitdir: /fake/repo/.git/worktrees/wt\n"
            )
            self.assertIsNone(guard.git_repo_root(outside_dir))

            cmd_rel = (
                'python3 -c \'p="src/lib.rs"; open(p, "w").write("pub fn f() {}\\n")\''
            )
            payload_in_wt = {
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_input": {"command": cmd_rel},
                "cwd": wt_root,
            }
            raw_in_wt, _ = self._run_hook(payload_in_wt)
            self.assertEqual(
                json.loads(raw_in_wt)["hookSpecificOutput"]["permissionDecision"],
                "deny",
            )

            cd_commands = [
                f"cd {wt_root} && {cmd_rel}",
                f"cd -- {wt_root}; {cmd_rel}",
                f"(cd {wt_root} && {cmd_rel})",
                f"env -C {wt_root} {cmd_rel}",
                f'bash -lc \'cd {wt_root} && python3 -c "open(\\"src/lib.rs\\", \\"w\\").write(\\"x\\")"\'',
                f'python3 -c \'open("{wt_root}/src/lib.rs", "w").write("x")\'',
            ]
            for command in cd_commands:
                with self.subTest(command=command):
                    payload = {
                        "hook_event_name": "PreToolUse",
                        "tool_name": "Bash",
                        "tool_input": {"command": command},
                        "cwd": outside_dir,
                    }
                    raw_out, _ = self._run_hook(payload)
                    self.assertEqual(
                        json.loads(raw_out)["hookSpecificOutput"]["permissionDecision"],
                        "deny",
                    )

            subshell_then_repo = (
                f"(cd {outside_dir} && echo scratch); "
                'python3 -c \'open("src/lib.rs", "w").write("x")\''
            )
            self.assertEqual(
                guard.scripted_source_write_target(subshell_then_repo, self.cwd),
                "src/lib.rs",
            )
