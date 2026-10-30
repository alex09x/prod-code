# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Shared loader, fixtures, and support mixin for prod-code hook installer tests."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import TYPE_CHECKING

INSTALLER_PATH = Path(__file__).resolve().parent / "hooks" / "install-hooks.py"
spec = importlib.util.spec_from_file_location("installer", str(INSTALLER_PATH))
assert spec is not None and spec.loader is not None
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class _FakeHttpResponse:
    def __init__(self, body: bytes) -> None:
        self._body = body

    def read(self) -> bytes:
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> bool:
        return False


def _write_version_bundle(
    bundle_dir: Path, tag: str, pause_on_stdin: bool = False
) -> Path:
    bundle_dir.mkdir(parents=True, exist_ok=True)
    for rel in installer.REQUIRED_GUARD_RELPATHS:
        mod_file = bundle_dir / rel
        mod_file.parent.mkdir(parents=True, exist_ok=True)
        mod_file.write_text(f'VERSION = "{tag}"\nREL = "{rel}"\n', encoding="utf-8")

    entry = bundle_dir / installer.CANONICAL_HOOK_NAME
    pause_code = (
        'sys.stdout.write("READY\\n")\nsys.stdout.flush()\nsys.stdin.readline()\n'
        if pause_on_stdin
        else ""
    )
    modules = [
        ".".join(Path(rel).with_suffix("").parts)
        for rel in installer.REQUIRED_GUARD_RELPATHS
        if Path(rel).name != "__init__.py"
    ]
    entry.write_text(
        "#!/usr/bin/env python3\n"
        "import importlib\nimport sys\nfrom pathlib import Path\n"
        "_HOOK_DIR = str(Path(__file__).resolve().parent)\n"
        "if _HOOK_DIR not in sys.path:\n    sys.path.insert(0, _HOOK_DIR)\n"
        f'EXPECTED = "{tag}"\n'
        "from prod_code_guard import grep_guard\n"
        f"{pause_code}"
        "import prod_code_guard\n"
        f"MODULES = {modules!r}\n"
        "versions = {prod_code_guard.VERSION, grep_guard.VERSION}\n"
        "versions.update(importlib.import_module(name).VERSION for name in MODULES)\n"
        "if versions != {EXPECTED}:\n"
        "    print(f'MIXED:{EXPECTED}:{sorted(versions)}', file=sys.stderr)\n"
        "    sys.exit(2)\n"
        "print(f'HOMOGENEOUS:{EXPECTED}')\n",
        encoding="utf-8",
    )
    return entry


if TYPE_CHECKING:
    _CaseMixinBase = unittest.TestCase
else:
    _CaseMixinBase = object


class InstallerTestSupportMixin(_CaseMixinBase):
    tmpdir: tempfile.TemporaryDirectory[str]
    base: Path
    dummy_src: Path
    dummy_dest: Path

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

    def _assert_no_temp_artifacts(self, root: Path) -> None:
        if not root.exists():
            return
        leftovers = [
            p
            for p in root.rglob("*")
            if ".tmp." in p.name or p.name.startswith(".staging.")
        ]
        self.assertEqual(leftovers, [], f"Leftover staging artifacts: {leftovers}")

    def _run_hook(self, dest_hook: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(dest_hook)],
            text=True,
            capture_output=True,
            check=False,
        )
