#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Unit tests for prod-code hook installer (scripts/hooks/install-hooks.py)."""

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
from unittest import mock

INSTALLER_PATH = Path(__file__).resolve().parent / "hooks" / "install-hooks.py"
spec = importlib.util.spec_from_file_location("installer", str(INSTALLER_PATH))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class _FakeHttpResponse:
    def __init__(self, body: bytes) -> None:
        self._body = body

    def read(self) -> bytes:
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb) -> bool:
        return False


class InstallHooksTests(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.base = Path(self.tmpdir.name)

        # Create dummy source script and required guard package modules
        self.dummy_src = self.base / installer.CANONICAL_HOOK_NAME
        self.dummy_src.write_text("#!/usr/bin/env python3\nprint('dummy guard')\n", encoding="utf-8")
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

    def test_claude_provider_install_upgrade_and_check(self):
        claude = installer.ClaudeCodeProvider()
        claude.home_dir = str(self.base / ".claude")
        claude.config_file = str(self.base / ".claude" / "settings.json")

        os.makedirs(claude.home_dir, exist_ok=True)
        # Pre-seed with legacy matcher "Bash"
        installer.atomic_write_json(claude.config_file, {
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [{"type": "command", "command": str(self.dummy_dest)}],
                    }
                ]
            }
        })

        # Check reports outdated matcher
        ok, msg = claude.check(str(self.dummy_dest))
        self.assertFalse(ok)
        self.assertIn("Outdated matcher", msg)

        # Install upgrades matcher to Bash|Edit
        res = claude.install(str(self.dummy_dest))
        self.assertIn("Updated", res)

        # Check reports OK
        ok, msg = claude.check(str(self.dummy_dest))
        self.assertTrue(ok)
        self.assertIn("matcher: Bash|Edit", msg)

        # Uninstall removes hook
        un_res = claude.uninstall(str(self.dummy_dest))
        self.assertIn("Removed", un_res)
        ok_after, _ = claude.check(str(self.dummy_dest))
        self.assertFalse(ok_after)

    def test_claude_install_preserves_other_pre_tool_use_hooks(self):
        claude = installer.ClaudeCodeProvider()
        claude.home_dir = str(self.base / ".claude")
        claude.config_file = str(self.base / ".claude" / "settings.json")
        os.makedirs(claude.home_dir, exist_ok=True)

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
                    "PostToolUse": [
                        {
                            "matcher": "",
                            "hooks": [
                                {
                                    "type": "command",
                                    "command": "unrelated-post-tool-hook",
                                }
                            ],
                        }
                    ],
                }
            },
        )

        result = claude.install(str(self.dummy_dest))
        self.assertIn("Updated", result)
        with open(claude.config_file, encoding="utf-8") as config_file:
            configured = json.load(config_file)
        pre_tool_use = configured["hooks"]["PreToolUse"]
        self.assertEqual(len(pre_tool_use), 2)
        self.assertEqual(pre_tool_use[1], observer)
        self.assertEqual(
            configured["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "unrelated-post-tool-hook",
        )
        self.assertEqual(pre_tool_use[0]["matcher"], "Bash|Edit")
        self.assertEqual(pre_tool_use[0]["hooks"][0]["command"], str(self.dummy_dest))

    def test_codex_provider_install_and_check(self):
        codex = installer.CodexProvider()
        codex.home_dir = str(self.base / ".codex")
        codex.config_file = str(self.base / ".codex" / "hooks.json")

        os.makedirs(codex.home_dir, exist_ok=True)
        # Not installed yet
        ok, _msg = codex.check(str(self.dummy_dest))
        self.assertFalse(ok)

        # Install hook
        res = codex.install(str(self.dummy_dest))
        self.assertIn("Updated", res)

        ok, _msg = codex.check(str(self.dummy_dest))
        self.assertTrue(ok)

        # Uninstall hook
        un_res = codex.uninstall(str(self.dummy_dest))
        self.assertIn("Removed", un_res)
        ok_after, _ = codex.check(str(self.dummy_dest))
        self.assertFalse(ok_after)

    def test_antigravity_provider_install_and_check(self):
        agy = installer.AntigravityProvider()
        agy.home_dir = str(self.base / ".gemini")
        agy.config_file = str(self.base / ".gemini" / "config" / "hooks.json")

        os.makedirs(os.path.dirname(agy.config_file), exist_ok=True)
        # Not installed yet
        ok, msg = agy.check(str(self.dummy_dest))
        self.assertFalse(ok)

        # Install hook
        res = agy.install(str(self.dummy_dest))
        self.assertIn("Updated", res)

        ok, msg = agy.check(str(self.dummy_dest))
        self.assertTrue(ok)
        self.assertIn("matcher: run_command|replace_file_content", msg)

        # Uninstall hook
        un_res = agy.uninstall(str(self.dummy_dest))
        self.assertIn("Removed", un_res)
        ok_after, _ = agy.check(str(self.dummy_dest))
        self.assertFalse(ok_after)

    def test_deploy_canonical_script(self):
        installer.deploy_canonical_script(self.dummy_src, str(self.dummy_dest))
        self.assertTrue(os.path.isfile(self.dummy_dest))
        self.assertTrue(os.access(self.dummy_dest, os.X_OK))
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            mod_file = self.dummy_dest.parent / rel
            self.assertTrue(mod_file.is_file(), f"Missing deployed module {rel}")
            self.assertGreater(mod_file.stat().st_size, 0)
        ok, msg = installer.check_canonical_hook(str(self.dummy_dest))
        self.assertTrue(ok, msg)

    def test_normal_install_deploys_all_components_and_executes_standalone_guard(self):
        providers = self._isolated_providers()
        dest_hook = self.base / "canonical_hooks" / installer.CANONICAL_HOOK_NAME

        with contextlib.redirect_stdout(io.StringIO()):
            rc = installer.main(
                ["--hook-script", str(installer.REPO_HOOK_SRC), "--canonical-hook", str(dest_hook)],
                providers=providers,
            )
        self.assertEqual(rc, 0)
        self.assertTrue(dest_hook.is_file())
        self.assertTrue(os.access(dest_hook, os.X_OK))
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            deployed = dest_hook.parent / rel
            self.assertTrue(deployed.is_file(), f"Missing {rel}")
            self.assertEqual(
                deployed.read_bytes(),
                (installer.REPO_HOOK_SRC.parent / rel).read_bytes(),
            )

        with contextlib.redirect_stdout(io.StringIO()):
            check_rc = installer.main(
                ["--check", "--canonical-hook", str(dest_hook)],
                providers=providers,
            )
        self.assertEqual(check_rc, 0)

        # Execute the deployed standalone hook script in an isolated temporary Git repo
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
        # Claude Code Bash payload -> JSON hookSpecificOutput permissionDecision deny
        claude_proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            input=json.dumps({
                "tool_name": "Bash",
                "tool_input": {"command": rewrite_cmd},
                "cwd": str(repo),
            }),
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        self.assertEqual(claude_proc.returncode, 0, claude_proc.stderr)
        claude_out = json.loads(claude_proc.stdout)
        self.assertEqual(claude_out["hookSpecificOutput"]["permissionDecision"], "deny")
        self.assertIn(
            "Inline Python rewrites of repository source files are blocked",
            claude_out["hookSpecificOutput"]["permissionDecisionReason"],
        )

        # Antigravity run_command payload -> JSON decision deny
        agy_proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            input=json.dumps({
                "toolCall": {"name": "run_command", "args": {"CommandLine": rewrite_cmd, "Cwd": str(repo)}},
                "workspacePaths": [str(repo)],
            }),
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        self.assertEqual(agy_proc.returncode, 0, agy_proc.stderr)
        agy_out = json.loads(agy_proc.stdout)
        self.assertEqual(agy_out["decision"], "deny")
        self.assertIn("Inline Python rewrites of repository source files are blocked", agy_out["reason"])

        # Codex exec_command payload -> JSON hookSpecificOutput permissionDecision deny
        codex_proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            input=json.dumps({
                "tool_name": "exec_command",
                "tool_input": {"cmd": rewrite_cmd, "workdir": str(repo)},
            }),
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        self.assertEqual(codex_proc.returncode, 0, codex_proc.stderr)
        codex_out = json.loads(codex_proc.stdout)
        self.assertEqual(codex_out["hookSpecificOutput"]["permissionDecision"], "deny")

        # Allowed data-processing command -> exit 0 and empty stdout
        allow_proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            input=json.dumps({
                "tool_name": "Bash",
                "tool_input": {"command": "python3 -c 'import json; print(json.dumps({\"ok\": True}))'"},
                "cwd": str(repo),
            }),
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        self.assertEqual(allow_proc.returncode, 0, allow_proc.stderr)
        self.assertEqual(allow_proc.stdout.strip(), "")

    def test_health_check_fails_when_any_required_component_missing_or_empty(self):
        providers = self._isolated_providers()
        dest_hook = self.base / "health_hooks" / installer.CANONICAL_HOOK_NAME

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(
                    ["--hook-script", str(installer.REPO_HOOK_SRC), "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                0,
            )

        # Missing required submodule must fail health check even when provider configs are valid
        missing_mod = dest_hook.parent / "prod_code_guard" / "python_write_guard.py"
        saved_bytes = missing_mod.read_bytes()
        missing_mod.unlink()
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("prod_code_guard/python_write_guard.py", msg)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(["--check", "--canonical-hook", str(dest_hook)], providers=providers),
                1,
            )

        # Empty (0-byte) required submodule must also fail health check
        missing_mod.write_bytes(b"")
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("prod_code_guard/python_write_guard.py", msg)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(["--check", "--canonical-hook", str(dest_hook)], providers=providers),
                1,
            )

        # Restore module and verify non-executable / missing entrypoint also fail --check
        missing_mod.write_bytes(saved_bytes)
        dest_hook.chmod(0o644)
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("NOT EXECUTABLE", msg)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(["--check", "--canonical-hook", str(dest_hook)], providers=providers),
                1,
            )

        dest_hook.unlink()
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertFalse(ok)
        self.assertIn("MISSING", msg)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(["--check", "--canonical-hook", str(dest_hook)], providers=providers),
                1,
            )

    def test_update_repairs_legacy_single_file_install_and_outdated_modules(self):
        providers = self._isolated_providers()
        dest_hook = self.base / "legacy_hooks" / installer.CANONICAL_HOOK_NAME
        dest_hook.parent.mkdir(parents=True, exist_ok=True)
        dest_hook.write_text("#!/usr/bin/env python3\n# legacy single-file guard\n", encoding="utf-8")
        dest_hook.chmod(0o755)
        stale_shell = dest_hook.parent / "prod_code_guard" / "shell.py"
        stale_shell.parent.mkdir(parents=True, exist_ok=True)
        stale_shell.write_text("# outdated shell module\n", encoding="utf-8")
        for p in providers:
            p.install(str(dest_hook))

        # Before update, --check fails because required package modules are incomplete
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(["--check", "--canonical-hook", str(dest_hook)], providers=providers),
                1,
            )

        # Running normal install/update repairs entrypoint and all submodules
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(
                    ["--hook-script", str(installer.REPO_HOOK_SRC), "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                0,
            )
        self.assertEqual(dest_hook.read_bytes(), installer.REPO_HOOK_SRC.read_bytes())
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            self.assertEqual(
                (dest_hook.parent / rel).read_bytes(),
                (installer.REPO_HOOK_SRC.parent / rel).read_bytes(),
            )
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(["--check", "--canonical-hook", str(dest_hook)], providers=providers),
                0,
            )

    def test_fallback_download_fetches_all_required_components_and_rolls_back_on_partial_failure(self):
        missing_src = self.base / "missing_repo" / installer.CANONICAL_HOOK_NAME
        dest_hook = self.base / "downloaded_hooks" / installer.CANONICAL_HOOK_NAME
        base_url = "https://example.invalid/prod-code/hooks"
        requested_urls: list[str] = []

        def fake_urlopen_ok(url: str, timeout: int = 15) -> _FakeHttpResponse:
            requested_urls.append(url)
            rel = url[len(base_url) + 1 :]
            return _FakeHttpResponse(f"# downloaded {rel}\n".encode())

        with mock.patch("urllib.request.urlopen", side_effect=fake_urlopen_ok):
            installer.deploy_canonical_script(missing_src, str(dest_hook), base_url=base_url)

        expected_urls = [f"{base_url}/{rel}" for rel in installer.REQUIRED_GUARD_COMPONENTS]
        self.assertEqual(requested_urls, expected_urls)
        self.assertTrue(os.access(dest_hook, os.X_OK))
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            self.assertEqual(
                (dest_hook.parent / rel).read_text(encoding="utf-8"),
                f"# downloaded {rel}\n",
            )
        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertTrue(ok, msg)

        # Partial download failure must roll back staged files without corrupting existing install
        dest_hook.write_text("# existing entrypoint\n", encoding="utf-8")
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            (dest_hook.parent / rel).write_text(f"# existing {rel}\n", encoding="utf-8")

        def fake_urlopen_partial_fail(url: str, timeout: int = 15) -> _FakeHttpResponse:
            if url.endswith("python_write_guard.py"):
                raise OSError("simulated network timeout")
            return _FakeHttpResponse(b"# new content\n")

        with (
            mock.patch("urllib.request.urlopen", side_effect=fake_urlopen_partial_fail),
            self.assertRaises(FileNotFoundError),
        ):
            installer.deploy_canonical_script(missing_src, str(dest_hook), base_url=base_url)

        self.assertEqual(dest_hook.read_text(encoding="utf-8"), "# existing entrypoint\n")
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            self.assertEqual(
                (dest_hook.parent / rel).read_text(encoding="utf-8"),
                f"# existing {rel}\n",
            )
        leftover_tmps = [p for p in dest_hook.parent.rglob("*") if ".tmp." in p.name]
        self.assertEqual(leftover_tmps, [])

    def test_codex_and_antigravity_install_preserve_unrelated_hooks_and_settings(self):
        _, codex, agy = self._isolated_providers()
        observer = {
            "matcher": "",
            "hooks": [{"type": "command", "command": "observer-hook --pre"}],
        }
        post_hook = [{"matcher": "", "hooks": [{"type": "command", "command": "post-observer"}]}]

        installer.atomic_write_json(
            codex.config_file,
            {
                "customSetting": True,
                "hooks": {
                    "PreToolUse": [observer],
                    "PostToolUse": post_hook,
                },
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
                            "hooks": [{"type": "command", "command": str(self.dummy_dest)}],
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


if __name__ == "__main__":
    unittest.main()
