# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Unit tests for prod-code hook installer providers, CLI flags, and health checks."""

import contextlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

INSTALLER_PATH = Path(__file__).resolve().parent / "hooks" / "install-hooks.py"
spec = importlib.util.spec_from_file_location("installer", str(INSTALLER_PATH))
assert spec is not None and spec.loader is not None
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class InstallHooksTests(unittest.TestCase):
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

    def _isolated_providers(self) -> list:
        claude = installer.ClaudeCodeProvider()
        claude.home_dir = str(self.base / ".claude")
        claude.config_file = str(self.base / ".claude" / "settings.json")
        os.makedirs(claude.home_dir, exist_ok=True)

        codex = installer.CodexProvider()
        codex.home_dir = str(self.base / ".codex")
        codex.config_file = str(self.base / ".codex" / "hooks.json")
        os.makedirs(codex.home_dir, exist_ok=True)

        agy = installer.AntigravityProvider()
        agy.home_dir = str(self.base / ".gemini")
        agy.config_file = str(self.base / ".gemini" / "config" / "hooks.json")
        os.makedirs(os.path.dirname(agy.config_file), exist_ok=True)
        return [claude, codex, agy]

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

    def test_claude_provider_install_upgrade_and_check(self) -> None:
        claude, _, _ = self._isolated_providers()
        installer.atomic_write_json(
            claude.config_file,
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {"type": "command", "command": str(self.dummy_dest)}
                            ],
                        }
                    ]
                }
            },
        )
        ok, msg = claude.check(str(self.dummy_dest))
        self.assertFalse(ok)
        self.assertIn("Outdated matcher", msg)

        self.assertIn("Updated", claude.install(str(self.dummy_dest)))
        ok, msg = claude.check(str(self.dummy_dest))
        self.assertTrue(ok)
        self.assertIn("matcher: Bash|Edit", msg)

        self.assertIn("Removed", claude.uninstall(str(self.dummy_dest)))
        self.assertFalse(claude.check(str(self.dummy_dest))[0])

    def test_claude_install_preserves_other_pre_tool_use_hooks(self) -> None:
        claude, _, _ = self._isolated_providers()
        observer = {
            "matcher": "",
            "hooks": [
                {
                    "type": "command",
                    "command": "secondary-observer-hook ingest --event PreToolUse",
                    "timeout": 5,
                }
            ],
        }
        post_hook = [
            {
                "matcher": "",
                "hooks": [{"type": "command", "command": "unrelated-post-tool-hook"}],
            }
        ]
        installer.atomic_write_json(
            claude.config_file,
            {
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": str(self.dummy_dest),
                                    "timeout": 5,
                                    "statusMessage": "prod-code build guard",
                                }
                            ],
                        },
                        observer,
                    ],
                    "PostToolUse": post_hook,
                }
            },
        )

        self.assertIn("Updated", claude.install(str(self.dummy_dest)))
        configured = json.loads(Path(claude.config_file).read_text(encoding="utf-8"))
        pre_tool_use = configured["hooks"]["PreToolUse"]
        self.assertEqual(len(pre_tool_use), 2)
        self.assertEqual(pre_tool_use[1], observer)
        self.assertEqual(configured["hooks"]["PostToolUse"], post_hook)
        self.assertEqual(pre_tool_use[0]["matcher"], "Bash|Edit")
        self.assertEqual(pre_tool_use[0]["hooks"][0]["command"], str(self.dummy_dest))

    def test_codex_provider_install_and_check(self) -> None:
        _, codex, _ = self._isolated_providers()
        self.assertFalse(codex.check(str(self.dummy_dest))[0])
        self.assertIn("Updated", codex.install(str(self.dummy_dest)))
        self.assertTrue(codex.check(str(self.dummy_dest))[0])
        self.assertIn("Removed", codex.uninstall(str(self.dummy_dest)))
        self.assertFalse(codex.check(str(self.dummy_dest))[0])

    def test_antigravity_provider_install_and_check(self) -> None:
        _, _, agy = self._isolated_providers()
        self.assertFalse(agy.check(str(self.dummy_dest))[0])
        self.assertIn("Updated", agy.install(str(self.dummy_dest)))
        ok, msg = agy.check(str(self.dummy_dest))
        self.assertTrue(ok)
        self.assertIn("matcher: run_command|replace_file_content", msg)
        self.assertIn("Removed", agy.uninstall(str(self.dummy_dest)))
        self.assertFalse(agy.check(str(self.dummy_dest))[0])

    def test_codex_and_antigravity_install_preserve_unrelated_hooks_and_settings(
        self,
    ) -> None:
        _, codex, agy = self._isolated_providers()
        observer = {
            "matcher": "",
            "hooks": [{"type": "command", "command": "observer-hook --pre"}],
        }
        post_hook = [
            {"matcher": "", "hooks": [{"type": "command", "command": "post-observer"}]}
        ]

        installer.atomic_write_json(
            codex.config_file,
            {
                "customSetting": True,
                "hooks": {"PreToolUse": [observer], "PostToolUse": post_hook},
            },
        )
        codex.install(str(self.dummy_dest))
        codex_cfg = json.loads(Path(codex.config_file).read_text(encoding="utf-8"))
        self.assertTrue(codex_cfg["customSetting"])
        self.assertEqual(codex_cfg["hooks"]["PostToolUse"], post_hook)
        self.assertIn(observer, codex_cfg["hooks"]["PreToolUse"])
        self.assertTrue(codex.check(str(self.dummy_dest))[0])

        installer.atomic_write_json(
            agy.config_file,
            {
                "version": 1,
                "other-plugin": {
                    "enabled": True,
                    "PreToolUse": [observer],
                    "PostToolUse": post_hook,
                },
                "prod-code-guard": {
                    "enabled": True,
                    "PreToolUse": [
                        {
                            "matcher": "run_command",
                            "hooks": [
                                {"type": "command", "command": str(self.dummy_dest)}
                            ],
                        },
                        observer,
                    ],
                },
            },
        )
        self.assertFalse(agy.check(str(self.dummy_dest))[0])
        agy.install(str(self.dummy_dest))
        agy_cfg = json.loads(Path(agy.config_file).read_text(encoding="utf-8"))
        self.assertEqual(agy_cfg["version"], 1)
        self.assertEqual(agy_cfg["other-plugin"]["PostToolUse"], post_hook)
        self.assertEqual(agy_cfg["other-plugin"]["PreToolUse"], [observer])
        self.assertEqual(agy_cfg["prod-code-guard"]["PreToolUse"][1], observer)
        self.assertEqual(
            agy_cfg["prod-code-guard"]["PreToolUse"][0]["matcher"],
            "run_command|replace_file_content",
        )
        self.assertTrue(agy.check(str(self.dummy_dest))[0])

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
                ["--hook-script", str(custom_src), "--canonical-hook", str(dest_hook)],
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
                ["--uninstall", "--canonical-hook", str(dest_hook)], providers=providers
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
                    ["--check", "--canonical-hook", str(dest_hook)], providers=providers
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
                    ["--check", "--canonical-hook", str(dest_hook)], providers=providers
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
                    ["--check", "--canonical-hook", str(dest_hook)], providers=providers
                ),
                1,
            )


if __name__ == "__main__":
    unittest.main()
