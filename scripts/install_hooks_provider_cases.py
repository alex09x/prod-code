# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Provider registration, upgrade, and settings-preservation test cases."""

from __future__ import annotations

import json
import sys
from pathlib import Path

try:
    from scripts.installer_test_support import InstallerTestSupportMixin, installer
except ModuleNotFoundError:  # pragma: no cover
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from scripts.installer_test_support import InstallerTestSupportMixin, installer


class InstallHooksProviderCasesMixin(InstallerTestSupportMixin):
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
