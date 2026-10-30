# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Atomicity, failure-injection, concurrency, and CWD-isolation tests for hook deployment."""

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path

try:
    from scripts.hook_deploy_failure_cases import HookDeployFailureCasesMixin
    from scripts.installer_test_support import (
        INSTALLER_PATH,
        _FakeHttpResponse,
        _write_version_bundle,
        installer,
    )
except ModuleNotFoundError:  # pragma: no cover
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from scripts.hook_deploy_failure_cases import HookDeployFailureCasesMixin
    from scripts.installer_test_support import (
        INSTALLER_PATH,
        _FakeHttpResponse,
        _write_version_bundle,
        installer,
    )

__all__ = [
    "INSTALLER_PATH",
    "HookDeployAtomicityTests",
    "_FakeHttpResponse",
    "_write_version_bundle",
    "installer",
]


class HookDeployAtomicityTests(HookDeployFailureCasesMixin, unittest.TestCase):
    def setUp(self) -> None:
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.base = Path(self.tmpdir.name)

    def test_update_repairs_legacy_single_file_install_and_ignores_stale_outer_modules(
        self,
    ) -> None:
        providers = self._isolated_providers()
        dest_hook = self.base / "legacy_hooks" / installer.CANONICAL_HOOK_NAME
        dest_hook.parent.mkdir(parents=True, exist_ok=True)
        dest_hook.write_text(
            "#!/usr/bin/env python3\nprint('LEGACY')\n", encoding="utf-8"
        )
        dest_hook.chmod(0o755)

        stale_pkg = dest_hook.parent / "prod_code_guard"
        stale_pkg.mkdir(parents=True, exist_ok=True)
        (stale_pkg / "__init__.py").write_text(
            "raise RuntimeError('STALE_OUTER')\n", encoding="utf-8"
        )
        (stale_pkg / "shell.py").write_text(
            "raise RuntimeError('STALE_OUTER')\n", encoding="utf-8"
        )
        for p in providers:
            p.install(str(dest_hook))

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                installer.main(
                    ["--check", "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                1,
            )
            v2_src = _write_version_bundle(self.base / "v2_repair", "v2")
            self.assertEqual(
                installer.main(
                    [
                        "--hook-script",
                        str(v2_src),
                        "--canonical-hook",
                        str(dest_hook),
                    ],
                    providers=providers,
                ),
                0,
            )
            self.assertEqual(
                installer.main(
                    ["--check", "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                0,
            )

        proc = self._run_hook(dest_hook)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.strip(), "HOMOGENEOUS:v2")

    def test_running_and_concurrent_hook_processes_never_observe_mixed_versions(
        self,
    ) -> None:
        dest_hook = self.base / "concurrent_hooks" / installer.CANONICAL_HOOK_NAME
        v1_paused = _write_version_bundle(
            self.base / "v1_paused", "v1", pause_on_stdin=True
        )
        v2_src = _write_version_bundle(self.base / "v2_live", "v2")
        installer.deploy_canonical_script(v1_paused, str(dest_hook))

        inflight = subprocess.Popen(
            [sys.executable, str(dest_hook)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        assert inflight.stdout is not None and inflight.stdin is not None
        self.assertEqual(inflight.stdout.readline().strip(), "READY")

        installer.deploy_canonical_script(v2_src, str(dest_hook))
        self.assertEqual(self._run_hook(dest_hook).stdout.strip(), "HOMOGENEOUS:v2")

        inflight.stdin.write("resume\n")
        inflight.stdin.flush()
        out, err = inflight.communicate(timeout=10)
        self.assertEqual(inflight.returncode, 0, err)
        self.assertEqual(out.strip(), "HOMOGENEOUS:v1")

        sources = [
            _write_version_bundle(self.base / f"roll_{i}", f"v{i}")
            for i in range(3, 10)
        ]
        stop = threading.Event()
        failures: list[str] = []

        def worker() -> None:
            while not stop.is_set():
                proc = self._run_hook(dest_hook)
                if proc.returncode != 0 or not proc.stdout.strip().startswith(
                    "HOMOGENEOUS:v"
                ):
                    failures.append(
                        f"rc={proc.returncode} out={proc.stdout!r} err={proc.stderr!r}"
                    )

        threads = [threading.Thread(target=worker) for _ in range(2)]
        for t in threads:
            t.start()
        try:
            for src in sources:
                installer.deploy_canonical_script(src, str(dest_hook))
        finally:
            stop.set()
            for t in threads:
                t.join(timeout=10)

        self.assertEqual(failures, [])
        self.assertEqual(self._run_hook(dest_hook).stdout.strip(), "HOMOGENEOUS:v9")

    def test_no_imports_from_current_working_directory(self) -> None:
        hostile_cwd = self.base / "hostile_cwd"
        for rel in installer.REQUIRED_GUARD_RELPATHS:
            p = hostile_cwd / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(
                "raise RuntimeError('IMPORTED_GUARD_FROM_CWD')\n", encoding="utf-8"
            )
        for helper in ("hook_providers.py", "hook_deploy.py"):
            (hostile_cwd / helper).write_text(
                "raise RuntimeError('IMPORTED_INSTALLER_FROM_CWD')\n", encoding="utf-8"
            )

        dest_hook = self.base / "isolated_dest" / installer.CANONICAL_HOOK_NAME
        fake_home = self.base / "fake_home"
        (fake_home / ".claude").mkdir(parents=True)
        env = dict(os.environ, HOME=str(fake_home), PYTHONPATH=f".:{hostile_cwd}")
        env["PROD_CODE_GUARD_LOG"] = str(self.base / "guard.jsonl")
        env.pop("PROD_CODE_LOCAL_BUILD", None)

        install_proc = subprocess.run(
            [
                sys.executable,
                str(INSTALLER_PATH),
                "--canonical-hook",
                str(dest_hook),
            ],
            cwd=str(hostile_cwd),
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(install_proc.returncode, 0, install_proc.stderr)

        repo = self.base / "repo"
        (repo / ".git").mkdir(parents=True)
        (repo / "src").mkdir()
        (repo / "src" / "lib.rs").write_text("pub fn f() {}\n", encoding="utf-8")
        hook_proc = subprocess.run(
            [sys.executable, str(dest_hook)],
            input=json.dumps(
                {
                    "tool_name": "Bash",
                    "tool_input": {"command": "sed -n '1,10p' src/lib.rs"},
                    "cwd": str(repo),
                }
            ),
            cwd=str(hostile_cwd),
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(hook_proc.returncode, 0, hook_proc.stderr)
        self.assertEqual(
            json.loads(hook_proc.stdout)["hookSpecificOutput"]["permissionDecision"],
            "deny",
        )


if __name__ == "__main__":
    unittest.main()
