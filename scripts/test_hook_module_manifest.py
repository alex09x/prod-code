# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Verification of guard runtime package metadata, installer bundle manifest, and bundle deployment."""

from __future__ import annotations

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

HOOKS_DIR = Path(__file__).resolve().parent / "hooks"
GUARD_PKG_DIR = HOOKS_DIR / "prod_code_guard"
INSTALLER_PATH = HOOKS_DIR / "install-hooks.py"

if str(HOOKS_DIR) not in sys.path:
    sys.path.insert(0, str(HOOKS_DIR))

import hook_deploy
import prod_code_guard

_installer_spec = importlib.util.spec_from_file_location(
    "install_hooks_manifest", INSTALLER_PATH
)
assert _installer_spec is not None and _installer_spec.loader is not None
installer = importlib.util.module_from_spec(_installer_spec)
_installer_spec.loader.exec_module(installer)


class HookModuleManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.base = Path(self.tmpdir.name)

    def _run_deployed_hook(
        self, hook_path: Path, payload: dict[str, object], cwd: Path | None = None
    ) -> subprocess.CompletedProcess[str]:
        env = dict(os.environ)
        env["PROD_CODE_GUARD_LOG"] = str(self.base / "guard.jsonl")
        env["PROD_CODE_RENAME_STATE_DIR"] = str(self.base / "renames")
        env.pop("PROD_CODE_LOCAL", None)
        env.pop("PROD_CODE_LOCAL_BUILD", None)
        env.pop("PROD_CODE_SOURCE_FALLBACK", None)
        return subprocess.run(
            [sys.executable, str(hook_path)],
            input=json.dumps(payload),
            cwd=str(cwd or self.base),
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_package_metadata_and_installer_manifest_match_all_runtime_modules(
        self,
    ) -> None:
        disk_modules = sorted(p.name for p in GUARD_PKG_DIR.glob("*.py"))
        self.assertEqual(
            sorted(prod_code_guard.GUARD_SUBMODULES),
            disk_modules,
            "GUARD_SUBMODULES must list every runtime module in prod_code_guard/",
        )
        self.assertEqual(
            len(prod_code_guard.GUARD_SUBMODULES),
            len(set(prod_code_guard.GUARD_SUBMODULES)),
            "GUARD_SUBMODULES must not contain duplicate entries",
        )
        expected_relpaths = tuple(
            f"prod_code_guard/{mod}" for mod in prod_code_guard.GUARD_SUBMODULES
        )
        self.assertEqual(hook_deploy.REQUIRED_GUARD_RELPATHS, expected_relpaths)
        self.assertEqual(installer.REQUIRED_GUARD_RELPATHS, expected_relpaths)
        self.assertEqual(
            hook_deploy.REQUIRED_GUARD_COMPONENTS,
            (hook_deploy.CANONICAL_HOOK_NAME, *expected_relpaths),
        )
        self.assertEqual(
            installer.REQUIRED_GUARD_COMPONENTS,
            hook_deploy.REQUIRED_GUARD_COMPONENTS,
        )

    def test_installed_bundle_includes_all_runtime_modules_and_executes_decomposed_paths(
        self,
    ) -> None:
        dest_hook = self.base / "installed" / installer.CANONICAL_HOOK_NAME
        installer.deploy_canonical_script(installer.REPO_HOOK_SRC, str(dest_hook))

        ok, msg = installer.check_canonical_hook(str(dest_hook))
        self.assertTrue(ok, msg)

        active_dir = installer.resolve_active_version_dir(dest_hook)
        self.assertIsNotNone(active_dir)
        assert active_dir is not None
        staged_pkg = active_dir / "prod_code_guard"
        self.assertEqual(
            sorted(p.name for p in staged_pkg.glob("*.py")),
            sorted(prod_code_guard.GUARD_SUBMODULES),
        )

        repo = self.base / "repo"
        (repo / ".git").mkdir(parents=True)
        (repo / "src").mkdir()
        (repo / "src" / "lib.rs").write_text("pub fn alpha() {}\n", encoding="utf-8")

        for cmd in (
            "sed -n '1,5p' src/lib.rs",
            "rg alpha_symbol src/",
            "python3 -c \"open('src/lib.rs', 'w').write('x')\"",
        ):
            proc = self._run_deployed_hook(
                dest_hook,
                {"tool_name": "Bash", "tool_input": {"command": cmd}, "cwd": str(repo)},
                cwd=repo,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            out = json.loads(proc.stdout)
            self.assertEqual(out["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_fallback_download_bundle_includes_all_runtime_modules_at_pinned_sha(
        self,
    ) -> None:
        missing_src = self.base / "missing" / hook_deploy.CANONICAL_HOOK_NAME
        dest_hook = self.base / "fallback" / hook_deploy.CANONICAL_HOOK_NAME
        sha = "b" * 40
        requested_urls: list[str] = []

        def fake_urlopen(url: str, timeout: int = 15) -> io.BytesIO:
            requested_urls.append(url)
            if url == "https://api.github.com/repos/alex09x/prod-code/commits/main":
                return io.BytesIO(json.dumps({"sha": sha}).encode("utf-8"))
            prefix = f"https://raw.githubusercontent.com/alex09x/prod-code/{sha}/scripts/hooks/"
            self.assertTrue(url.startswith(prefix), url)
            rel = url[len(prefix) :]
            return io.BytesIO((HOOKS_DIR / rel).read_bytes())

        with mock.patch.object(
            hook_deploy.urllib.request, "urlopen", side_effect=fake_urlopen
        ):
            hook_deploy.deploy_canonical_script(missing_src, str(dest_hook))

        self.assertEqual(
            requested_urls[1:],
            [
                f"https://raw.githubusercontent.com/alex09x/prod-code/{sha}/scripts/hooks/{rel}"
                for rel in hook_deploy.REQUIRED_GUARD_COMPONENTS
            ],
        )
        ok, msg = hook_deploy.check_canonical_hook(str(dest_hook))
        self.assertTrue(ok, msg)

        active_dir = hook_deploy.resolve_active_version_dir(dest_hook)
        assert active_dir is not None
        self.assertEqual(
            sorted(p.name for p in (active_dir / "prod_code_guard").glob("*.py")),
            sorted(prod_code_guard.GUARD_SUBMODULES),
        )

        repo = self.base / "repo"
        (repo / ".git").mkdir(parents=True)
        (repo / "src").mkdir()
        (repo / "src" / "lib.rs").write_text("pub fn beta() {}\n", encoding="utf-8")
        proc = self._run_deployed_hook(
            dest_hook,
            {
                "tool_name": "Bash",
                "tool_input": {"command": "python3 -c \"Path('src/lib.rs')\""},
                "cwd": str(repo),
            },
            cwd=repo,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.strip(), "")

    def test_installer_and_launcher_reject_poisoned_sibling_modules_in_cwd(
        self,
    ) -> None:
        hostile = self.base / "hostile_cwd"
        hostile.mkdir()
        for helper in ("hook_launcher.py", "hook_deploy.py", "hook_providers.py"):
            (hostile / helper).write_text(
                'raise RuntimeError("POISONED_INSTALLER_HELPER")\n', encoding="utf-8"
            )
        for rel in hook_deploy.REQUIRED_GUARD_RELPATHS:
            target = hostile / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(
                'raise RuntimeError("POISONED_GUARD_SUBMODULE")\n', encoding="utf-8"
            )

        dest_hook = self.base / "safe_dest" / hook_deploy.CANONICAL_HOOK_NAME
        fake_home = self.base / "fake_home"
        (fake_home / ".claude").mkdir(parents=True)
        env = dict(os.environ, HOME=str(fake_home), PYTHONPATH=f".:{hostile}")
        env["PROD_CODE_GUARD_LOG"] = str(self.base / "guard.jsonl")

        install_proc = subprocess.run(
            [sys.executable, str(INSTALLER_PATH), "--canonical-hook", str(dest_hook)],
            cwd=str(hostile),
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(install_proc.returncode, 0, install_proc.stderr)

        hook_proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            input=json.dumps({"tool_name": "Bash", "tool_input": {"command": "true"}}),
            cwd=str(hostile),
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(hook_proc.returncode, 0, hook_proc.stderr)


if __name__ == "__main__":
    unittest.main()
