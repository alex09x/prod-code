# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Allowed scratch/non-source writes, fail-open Python cases, and hook file-size invariants."""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

from .scripted_write_denials import ScriptedSourceWriteFixtureBase
from .support import HOOKS_DIR


class ScriptedSourceWriteExemptionCasesMixin(ScriptedSourceWriteFixtureBase):
    def test_data_stdin_temp_scratch_ssh_sed_inplace_and_text_mentions_allowed(
        self,
    ) -> None:
        guard = self._guard()
        tmp_base = "/tmp" if os.path.isdir("/tmp") else None
        with tempfile.TemporaryDirectory(dir=tmp_base) as scratch_dir:
            self.assertIsNone(guard.git_repo_root(scratch_dir))
            scratch_py = str(Path(scratch_dir) / "probe.py")
            scratch_json = str(Path(scratch_dir) / "data.json")

            allowed_commands = [
                "python3 -c 'p=\"crates/prod-code-mcp/src/lib.rs\"; print(len(open(p).read()))'",
                "cat crates/prod-code-mcp/src/lib.rs | python3 -c 'import sys; print(sys.stdin.read().count(\"fn \"))'",
                'python3 -c \'import json; json.dump({"ok": True}, open("target/report.json", "w"))\'',
                'python3 -c \'from pathlib import Path; Path("docs/notes.md").write_text("# Notes\\n")\'',
                (
                    "python3 -c 'import json; "
                    'open("summary.json", "w").write(json.dumps({"source": "crates/prod-code-mcp/src/lib.rs"}))\''
                ),
                (
                    "python3 - <<'EOF'\n"
                    "src = 'crates/prod-code-mcp/src/lib.rs'\n"
                    "body = open(src).read()\n"
                    "open('analysis.json', 'w').write(f'{src}:{len(body)}')\n"
                    "EOF"
                ),
                (
                    "python3 - <<'EOF'\n"
                    "p = 'crates/prod-code-mcp/src/lib.rs'\n"
                    "_ = open(p).read()\n"
                    "p = 'analysis.json'\n"
                    "open(p, 'w').write('{\"ok\": true}\\n')\n"
                    "EOF"
                ),
                (
                    "python3 - <<'EOF'\n"
                    "from pathlib import Path\n"
                    "def save(dest, text):\n"
                    "    Path(dest).write_text(text)\n"
                    "save('report.txt', 'crates/prod-code-mcp/src/lib.rs')\n"
                    "EOF"
                ),
                (
                    "python3 -m json.tool <<'EOF'\n"
                    '{"file": "crates/prod-code-mcp/src/lib.rs"}\n'
                    "EOF"
                ),
                (
                    "python3 -c 'import sys; open(\"out.txt\", \"w\").write(sys.stdin.read())' <<'EOF'\n"
                    "open('crates/prod-code-mcp/src/lib.rs', 'w').write('not executed')\n"
                    "EOF"
                ),
                (
                    "python3 scripts/some_tool.py <<'EOF'\n"
                    "open('crates/prod-code-mcp/src/lib.rs', 'w').write('stdin data')\n"
                    "EOF"
                ),
                f'python3 -c \'open("{scratch_json}", "w").write("{{}}")\'',
                f'python3 -c \'open("{scratch_py}", "w").write("print(1)\\n")\'',
                f'cd {scratch_dir} && python3 -c \'open("scratch_helper.py", "w").write("x = 1\\n")\'',
                'python3 -c \'import tempfile; f = tempfile.NamedTemporaryFile("w", suffix=".py", delete=False); f.write("x=1\\n")\'',
                'python3 -c \'import tempfile; from pathlib import Path\nwith tempfile.TemporaryDirectory() as d:\n    Path(d, "repro.rs").write_text("fn main() {}\\n")\'',
                "python3 -c \"open('$TMPDIR/scratch.py', 'w').write('x = 1\\n')\"",
                'ssh remote-node "python3 -c \'p=\\"src/lib.rs\\"; open(p, \\"w\\").write(\\"x\\")\'"',
                'ssh remote-node python3 -c \'open("src/lib.rs", "w").write("x")\'',
                "sed -i '' 's/old/new/' src/main.rs",
                "sed -i.bak 's/old/new/' src/main.rs",
            ]
            for command in allowed_commands:
                with self.subTest(command=command):
                    payload = {
                        "hook_event_name": "PreToolUse",
                        "tool_name": "Bash",
                        "tool_input": {"command": command},
                        "cwd": self.cwd,
                    }
                    raw_out, _ = self._run_hook(payload)
                    self.assertEqual(raw_out, "", command)

    def test_opaque_or_ambiguous_python_programs_fail_open(self) -> None:
        opaque_commands = [
            'python3 -c \'import os; p = os.environ["TARGET_PATH"]; open(p, "w").write("x")\'',
            'python3 -c \'import glob; [open(p, "w").write("x") for p in glob.glob("out/*")]\'',
            'python3 -c \'p = "src/lib.rs"; import glob; [open(p, "w").write("x") for p in glob.glob("out/*")]\'',
            'python3 -c \'import os; p = "src/lib.rs" if os.getenv("A") else "out.json"; open(p, "w").write("x")\'',
            "python3 -c 'exec(os.environ[\"DYNAMIC_CODE\"])'",
            'd=$(mktemp -d) && cd "$d" && python3 -c \'open("generated.py", "w").write("x = 1\\n")\'',
            "python3 - <<\\EOF\ns = '`grep -rn someIdentifier src/`'\nEOF\n",
            "python3 - <<EOF\ndoc = '`rg my_symbol crates/`'\nEOF\n",
        ]
        for command in opaque_commands:
            with self.subTest(command=command):
                payload = {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Bash",
                    "tool_input": {"command": command},
                    "cwd": self.cwd,
                }
                raw_out, _ = self._run_hook(payload)
                self.assertEqual(raw_out, "", command)

    def test_all_hook_source_files_stay_within_compact_400_line_ceiling(self) -> None:
        hooks_dir = HOOKS_DIR
        py_files = sorted(hooks_dir.rglob("*.py"))
        self.assertGreaterEqual(len(py_files), 8)
        for py_file in py_files:
            line_count = len(py_file.read_text(encoding="utf-8").splitlines())
            self.assertLessEqual(
                line_count,
                400,
                f"{py_file.relative_to(hooks_dir)} exceeds 400 lines ({line_count})",
            )
