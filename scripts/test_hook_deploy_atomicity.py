# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Atomicity, failure-injection, concurrency, and CWD-isolation tests for hook deployment."""

import contextlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

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
    entry.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\nfrom pathlib import Path\n"
        "_HOOK_DIR = str(Path(__file__).resolve().parent)\n"
        "if _HOOK_DIR not in sys.path:\n    sys.path.insert(0, _HOOK_DIR)\n"
        f'EXPECTED = "{tag}"\n'
        "from prod_code_guard import grep_guard\n"
        f"{pause_code}"
        "import prod_code_guard\n"
        "from prod_code_guard import python_write_guard, rename_guard, sed_guard, shell, shell_python\n"
        "versions = {\n"
        "    prod_code_guard.VERSION, grep_guard.VERSION, python_write_guard.VERSION,\n"
        "    rename_guard.VERSION, sed_guard.VERSION, shell.VERSION, shell_python.VERSION,\n"
        "}\n"
        "if versions != {EXPECTED}:\n"
        "    print(f'MIXED:{EXPECTED}:{sorted(versions)}', file=sys.stderr)\n"
        "    sys.exit(2)\n"
        "print(f'HOMOGENEOUS:{EXPECTED}')\n",
        encoding="utf-8",
    )
    return entry


class HookDeployAtomicityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.base = Path(self.tmpdir.name)

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

    def _inject_every_call_failure(
        self,
        target_attr: str,
        real_fn: object,
        steady_state: bool,
        run_deploy: object,
        verify_intact: object,
    ) -> None:
        count = 0

        def counting_fn(*args: object, **kwargs: object) -> object:
            nonlocal count
            count += 1
            return real_fn(*args, **kwargs)  # type: ignore[operator]

        probe_dir = (
            self.base / f"probe_{target_attr.replace('.', '_')}_{os.urandom(3).hex()}"
        )
        probe_src = _write_version_bundle(probe_dir / "src", "probe")
        probe_dest = probe_dir / "dest" / installer.CANONICAL_HOOK_NAME
        if steady_state:
            installer.deploy_canonical_script(
                _write_version_bundle(probe_dir / "v0", "v0"), str(probe_dest)
            )
        with mock.patch(target_attr, side_effect=counting_fn):
            installer.deploy_canonical_script(probe_src, str(probe_dest))
        self.assertGreater(count, 0)

        def make_failing_fn(fail_idx: int) -> object:
            seen = 0

            def failing_fn(*args: object, **kwargs: object) -> object:
                nonlocal seen
                seen += 1
                if seen == fail_idx:
                    raise OSError(f"injected {target_attr} failure at call #{fail_idx}")
                return real_fn(*args, **kwargs)  # type: ignore[operator]

            return failing_fn

        for fail_on in range(1, count + 1):
            with (
                mock.patch(target_attr, side_effect=make_failing_fn(fail_on)),
                self.assertRaises(OSError),
            ):
                run_deploy()  # type: ignore[operator]
            verify_intact()  # type: ignore[operator]

    def test_second_component_and_every_step_failure_preserve_active_version(
        self,
    ) -> None:
        v1_src = _write_version_bundle(self.base / "v1", "v1")
        v2_src = _write_version_bundle(self.base / "v2", "v2")
        dest_hook = self.base / "installed" / installer.CANONICAL_HOOK_NAME

        installer.deploy_canonical_script(v1_src, str(dest_hook))
        self.assertEqual(self._run_hook(dest_hook).stdout.strip(), "HOMOGENEOUS:v1")

        def verify_v1_intact() -> None:
            proc = self._run_hook(dest_hook)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(proc.stdout.strip(), "HOMOGENEOUS:v1")
            ok, msg = installer.check_canonical_hook(str(dest_hook))
            self.assertTrue(ok, msg)
            self._assert_no_temp_artifacts(dest_hook.parent)

        for attr, fn in (
            ("shutil.copy2", shutil.copy2),
            ("os.chmod", os.chmod),
            ("os.symlink", os.symlink),
            ("os.replace", os.replace),
        ):
            self._inject_every_call_failure(
                attr,
                fn,
                True,
                lambda: installer.deploy_canonical_script(v2_src, str(dest_hook)),
                verify_v1_intact,
            )

        installer.deploy_canonical_script(v2_src, str(dest_hook))
        self.assertEqual(self._run_hook(dest_hook).stdout.strip(), "HOMOGENEOUS:v2")

    def test_failures_preserve_first_install_and_legacy_single_file_upgrade(
        self,
    ) -> None:
        v2_src = _write_version_bundle(self.base / "v2", "v2")
        first_dest = self.base / "first_install" / installer.CANONICAL_HOOK_NAME

        def verify_first_clean() -> None:
            self.assertFalse(first_dest.exists() or first_dest.is_symlink())
            self.assertFalse(installer.versions_store_dir(first_dest).exists())
            self._assert_no_temp_artifacts(first_dest.parent)

        for attr, fn in (
            ("shutil.copy2", shutil.copy2),
            ("os.chmod", os.chmod),
            ("os.symlink", os.symlink),
            ("os.replace", os.replace),
        ):
            self._inject_every_call_failure(
                attr,
                fn,
                False,
                lambda: installer.deploy_canonical_script(v2_src, str(first_dest)),
                verify_first_clean,
            )

        legacy_dest = self.base / "legacy_install" / installer.CANONICAL_HOOK_NAME
        legacy_dest.parent.mkdir(parents=True, exist_ok=True)
        legacy_dest.write_text(
            "#!/usr/bin/env python3\nprint('LEGACY_SINGLE_FILE')\n", encoding="utf-8"
        )
        legacy_dest.chmod(0o755)

        def verify_legacy_usable() -> None:
            proc = self._run_hook(legacy_dest)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(proc.stdout.strip(), "LEGACY_SINGLE_FILE")
            self._assert_no_temp_artifacts(legacy_dest.parent)

        for attr, fn in (
            ("shutil.copy2", shutil.copy2),
            ("os.chmod", os.chmod),
            ("os.symlink", os.symlink),
            ("os.replace", os.replace),
        ):
            self._inject_every_call_failure(
                attr,
                fn,
                False,
                lambda: installer.deploy_canonical_script(v2_src, str(legacy_dest)),
                verify_legacy_usable,
            )

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
                    ["--check", "--canonical-hook", str(dest_hook)], providers=providers
                ),
                1,
            )
            v2_src = _write_version_bundle(self.base / "v2_repair", "v2")
            self.assertEqual(
                installer.main(
                    ["--hook-script", str(v2_src), "--canonical-hook", str(dest_hook)],
                    providers=providers,
                ),
                0,
            )
            self.assertEqual(
                installer.main(
                    ["--check", "--canonical-hook", str(dest_hook)], providers=providers
                ),
                0,
            )

        proc = self._run_hook(dest_hook)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.strip(), "HOMOGENEOUS:v2")

    def test_fallback_download_fetches_all_components_and_preserves_previous_on_failure(
        self,
    ) -> None:
        missing_src = self.base / "missing_repo" / installer.CANONICAL_HOOK_NAME
        dest_hook = self.base / "downloaded_hooks" / installer.CANONICAL_HOOK_NAME
        base_url = "https://example.invalid/prod-code/hooks"
        requested_urls: list[str] = []
        dl_v1 = self.base / "dl_v1_template"
        _write_version_bundle(dl_v1, "dl_v1")

        def fake_urlopen_ok(url: str, timeout: int = 15) -> _FakeHttpResponse:
            requested_urls.append(url)
            return _FakeHttpResponse((dl_v1 / url[len(base_url) + 1 :]).read_bytes())

        with mock.patch("urllib.request.urlopen", side_effect=fake_urlopen_ok):
            installer.deploy_canonical_script(
                missing_src, str(dest_hook), base_url=base_url
            )

        self.assertEqual(
            requested_urls,
            [f"{base_url}/{rel}" for rel in installer.REQUIRED_GUARD_COMPONENTS],
        )
        self.assertTrue(installer.check_canonical_hook(str(dest_hook))[0])
        self.assertEqual(self._run_hook(dest_hook).stdout.strip(), "HOMOGENEOUS:dl_v1")

        dl_v2 = self.base / "dl_v2_template"
        _write_version_bundle(dl_v2, "dl_v2")
        for bad_suffix, empty_resp in (
            ("shell.py", False),
            ("python_write_guard.py", False),
            ("rename_guard.py", True),
        ):

            def fake_urlopen_fail(
                url: str,
                timeout: int = 15,
                suffix: str = bad_suffix,
                empty: bool = empty_resp,
            ) -> _FakeHttpResponse:
                if url.endswith(suffix):
                    if empty:
                        return _FakeHttpResponse(b"")
                    raise OSError(f"simulated timeout on {suffix}")
                return _FakeHttpResponse(
                    (dl_v2 / url[len(base_url) + 1 :]).read_bytes()
                )

            with (
                mock.patch("urllib.request.urlopen", side_effect=fake_urlopen_fail),
                self.assertRaises(FileNotFoundError),
            ):
                installer.deploy_canonical_script(
                    missing_src, str(dest_hook), base_url=base_url
                )

            self.assertEqual(
                self._run_hook(dest_hook).stdout.strip(), "HOMOGENEOUS:dl_v1"
            )
            self.assertTrue(installer.check_canonical_hook(str(dest_hook))[0])
            self._assert_no_temp_artifacts(dest_hook.parent)

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
            [sys.executable, str(INSTALLER_PATH), "--canonical-hook", str(dest_hook)],
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
