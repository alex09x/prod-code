# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Unit tests for prod-code hook installer providers, CLI flags, and health checks."""

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

try:
    from scripts.install_hooks_provider_cases import InstallHooksProviderCasesMixin
    from scripts.installer_test_support import INSTALLER_PATH, installer
except ModuleNotFoundError:  # pragma: no cover
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from scripts.install_hooks_provider_cases import InstallHooksProviderCasesMixin
    from scripts.installer_test_support import INSTALLER_PATH, installer

__all__ = ["INSTALLER_PATH", "InstallHooksTests", "installer"]


class InstallHooksTests(InstallHooksProviderCasesMixin, unittest.TestCase):
    def setUp(self) -> None:
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.base = Path(self.tmpdir.name)

        self.dummy_src = self.base / installer.CANONICAL_HOOK_NAME
        self.dummy_src.write_text(
            "#!/usr/bin/env python3\nprint('dummy guard')\n", encoding="utf-8"
        )
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            mod_path = self.base / rel
            mod_path.parent.mkdir(parents=True, exist_ok=True)
            mod_path.write_text(f"# dummy {rel}\n", encoding="utf-8")
        self.dummy_dest = self.base / "installed" / installer.CANONICAL_HOOK_NAME

    def _invoke_guard(
        self, dest_hook: Path, payload: dict, env: dict
    ) -> subprocess.CompletedProcess[str]:
        proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            input=json.dumps(payload),
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return proc

    def test_deploy_canonical_script_and_custom_hook_script(self) -> None:
        custom_dir = self.base / "custom_bundle"
        custom_src = custom_dir / "custom-guard-entry.py"
        custom_dir.mkdir(parents=True)
        custom_src.write_text(
            "#!/usr/bin/env python3\nfrom prod_code_guard import shell\nprint('CUSTOM_OK')\n",
            encoding="utf-8",
        )
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            p = custom_dir / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(f"# custom {rel}\n", encoding="utf-8")

        providers = self._isolated_providers()
        dest_hook = self.base / "custom_installed" / installer.CANONICAL_HOOK_NAME
        with contextlib.redirect_stdout(io.StringIO()):
            rc = installer.main(
                [
                    "--hook-script",
                    str(custom_src),
                    "--canonical-hook",
                    str(dest_hook),
                ],
                providers=providers,
            )
        self.assertEqual(rc, 0)
        self.assertTrue(dest_hook.is_file() and os.access(dest_hook, os.X_OK))
        active_dir = installer.resolve_active_version_dir(dest_hook)
        self.assertIsNotNone(active_dir)
        assert active_dir is not None
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            self.assertTrue(
                (active_dir / rel).is_file(), f"Missing deployed module {rel}"
            )
        self.assertTrue(installer.check_canonical_hook(str(dest_hook))[0])

        proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.strip(), "CUSTOM_OK")

        with contextlib.redirect_stdout(io.StringIO()):
            un_rc = installer.main(
                ["--uninstall", "--canonical-hook", str(dest_hook)],
                providers=providers,
            )
        self.assertEqual(un_rc, 0)
        for p in providers:
            self.assertFalse(
                p.check(str(dest_hook))[0], f"{p.name} remained registered"
            )

    def test_normal_install_deploys_all_components_and_executes_standalone_guard(
        self,
    ) -> None:
        providers = self._isolated_providers()
        dest_hook = self.base / "canonical_hooks" / installer.CANONICAL_HOOK_NAME

        with contextlib.redirect_stdout(io.StringIO()):
            rc = installer.main(
                [
                    "--hook-script",
                    str(installer.REPO_HOOK_SRC),
                    "--canonical-hook",
                    str(dest_hook),
                ],
                providers=providers,
            )
        self.assertEqual(rc, 0)
        active_dir = installer.resolve_active_version_dir(dest_hook)
        assert active_dir is not None
        self.assertEqual(
            (active_dir / dest_hook.name).read_bytes(),
            installer.REPO_HOOK_SRC.read_bytes(),
        )
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            self.assertEqual(
                (active_dir / rel).read_bytes(),
                (installer.REPO_HOOK_SRC.parent / rel).read_bytes(),
            )

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(
                    ["--check", "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                0,
            )

        repo = self.base / "repo"
        (repo / ".git").mkdir(parents=True)
        (repo / "src").mkdir()
        (repo / "src" / "lib.rs").write_text("pub fn old_name() {}\n", encoding="utf-8")
        env = dict(os.environ)
        env.pop("PROD_CODE_LOCAL_BUILD", None)
        env["PROD_CODE_GUARD_LOG"] = str(self.base / "guard.jsonl")
        rewrite_cmd = (
            "python3 - <<'PY'\n"
            "from pathlib import Path\n"
            "Path('src/lib.rs').write_text('pub fn new_name() {}\\n')\n"
            "PY"
        )

        claude_out = json.loads(
            self._invoke_guard(
                dest_hook,
                {
                    "tool_name": "Bash",
                    "tool_input": {"command": rewrite_cmd},
                    "cwd": str(repo),
                },
                env,
            ).stdout
        )
        self.assertEqual(claude_out["hookSpecificOutput"]["permissionDecision"], "deny")
        self.assertIn(
            "Inline Python rewrites of repository source files are blocked",
            claude_out["hookSpecificOutput"]["permissionDecisionReason"],
        )

        agy_out = json.loads(
            self._invoke_guard(
                dest_hook,
                {
                    "toolCall": {
                        "name": "run_command",
                        "args": {"CommandLine": rewrite_cmd, "Cwd": str(repo)},
                    },
                    "workspacePaths": [str(repo)],
                },
                env,
            ).stdout
        )
        self.assertEqual(agy_out["decision"], "deny")

        codex_out = json.loads(
            self._invoke_guard(
                dest_hook,
                {
                    "tool_name": "exec_command",
                    "tool_input": {"cmd": rewrite_cmd, "workdir": str(repo)},
                },
                env,
            ).stdout
        )
        self.assertEqual(codex_out["hookSpecificOutput"]["permissionDecision"], "deny")

        allow_proc = self._invoke_guard(
            dest_hook,
            {
                "tool_name": "Bash",
                "tool_input": {
                    "command": "python3 -c 'import json; print(json.dumps({\"ok\": True}))'"
                },
                "cwd": str(repo),
            },
            env,
        )
        self.assertEqual(allow_proc.stdout.strip(), "")

    def test_health_check_fails_when_any_required_component_missing_or_empty(
        self,
    ) -> None:
        providers = self._isolated_providers()
        dest_hook = self.base / "health_hooks" / installer.CANONICAL_HOOK_NAME
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(
                    [
                        "--hook-script",
                        str(installer.REPO_HOOK_SRC),
                        "--canonical-hook",
                        str(dest_hook),
                    ],
                    providers=providers,
                ),
                0,
            )

        active_dir = installer.resolve_active_version_dir(dest_hook)
        assert active_dir is not None
        missing_mod = active_dir / "prod_code_guard" / "python_write_guard.py"
        saved_bytes = missing_mod.read_bytes()

        missing_mod.unlink()
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("prod_code_guard/python_write_guard.py", msg)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(
                    ["--check", "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                1,
            )

        missing_mod.write_bytes(b"")
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("prod_code_guard/python_write_guard.py", msg)

        missing_mod.write_bytes(saved_bytes)
        dest_hook.chmod(0o644)
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("NOT EXECUTABLE", msg)

        dest_hook.unlink()
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("MISSING", msg)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(
                    ["--check", "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                1,
            )


if __name__ == "__main__":
    unittest.main()
