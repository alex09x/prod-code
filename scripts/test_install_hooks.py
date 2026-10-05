#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Unit tests for prod-code hook installer (scripts/hooks/install-hooks.py)."""

import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path

INSTALLER_PATH = Path(__file__).resolve().parent / "hooks" / "install-hooks.py"
spec = importlib.util.spec_from_file_location("installer", str(INSTALLER_PATH))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class InstallHooksTests(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.base = Path(self.tmpdir.name)

        # Create dummy source script
        self.dummy_src = self.base / installer.CANONICAL_HOOK_NAME
        self.dummy_src.write_text("#!/usr/bin/env python3\nprint('dummy guard')\n")
        self.dummy_dest = self.base / "installed" / installer.CANONICAL_HOOK_NAME

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

    def test_codex_provider_install_and_check(self):
        codex = installer.CodexProvider()
        codex.home_dir = str(self.base / ".codex")
        codex.config_file = str(self.base / ".codex" / "hooks.json")

        os.makedirs(codex.home_dir, exist_ok=True)
        # Not installed yet
        ok, msg = codex.check(str(self.dummy_dest))
        self.assertFalse(ok)

        # Install hook
        res = codex.install(str(self.dummy_dest))
        self.assertIn("Updated", res)

        ok, msg = codex.check(str(self.dummy_dest))
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


if __name__ == "__main__":
    unittest.main()
