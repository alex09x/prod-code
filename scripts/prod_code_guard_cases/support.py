# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Shared hook loader, repository root fixture, and stdio helpers for guard tests."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from collections.abc import Iterator, Mapping
from pathlib import Path
from typing import TYPE_CHECKING, Any

SCRIPTS_DIR = Path(__file__).resolve().parent.parent
REPO_DIR = SCRIPTS_DIR.parent
HOOKS_DIR = SCRIPTS_DIR / "hooks"
HOOK_SCRIPT = HOOKS_DIR / "prod-code-local-build-guard.py"

spec = importlib.util.spec_from_file_location("guard", str(HOOK_SCRIPT))
if spec is None or spec.loader is None:
    raise RuntimeError(f"Unable to load guard module spec from {HOOK_SCRIPT}")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


def make_git_repo_root(testcase: unittest.TestCase) -> Path:
    checkout = REPO_DIR
    if (checkout / ".git").exists():
        return checkout
    tmp = tempfile.TemporaryDirectory()
    testcase.addCleanup(tmp.cleanup)
    root = Path(tmp.name)
    (root / ".git").mkdir()
    (root / "scripts" / "hooks").mkdir(parents=True)
    lib_rs = root / "crates" / "prod-code-mcp" / "src" / "lib.rs"
    lib_rs.parent.mkdir(parents=True)
    _ = lib_rs.write_text("// stub for remote workspace mirror without .git\n", encoding="utf-8")
    return root


if TYPE_CHECKING:
    _TestCaseMixinBase = unittest.TestCase
else:
    _TestCaseMixinBase = object


class GuardTestCaseBase(_TestCaseMixinBase):
    """Non-TestCase runtime base for guard test mixins with monkeypatch-aware lookup."""

    def _guard(self) -> Any:
        for cls in self.__class__.__mro__:
            mod = sys.modules.get(cls.__module__)
            if mod is not None and "guard" in mod.__dict__:
                return mod.__dict__["guard"]
        return guard

    def _invoke_main_stdio(self, payload: Mapping[str, object]) -> str:
        active_guard = self._guard()
        stdout = io.StringIO()
        orig_stdin, orig_stdout = sys.stdin, sys.stdout
        try:
            sys.stdin = io.StringIO(json.dumps(payload))
            sys.stdout = stdout
            active_guard.main()
        finally:
            sys.stdin, sys.stdout = orig_stdin, orig_stdout
        return stdout.getvalue().strip()

    @contextlib.contextmanager
    def _temporary_rename_state_dir(self) -> Iterator[str]:
        active_guard = self._guard()
        with tempfile.TemporaryDirectory() as tmpdir:
            orig_dir = active_guard.RENAME_STATE_DIR
            try:
                active_guard.RENAME_STATE_DIR = tmpdir
                yield tmpdir
            finally:
                active_guard.RENAME_STATE_DIR = orig_dir
