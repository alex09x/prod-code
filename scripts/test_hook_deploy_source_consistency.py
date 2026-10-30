"""Regression for locally supplied incomplete guard bundles."""

# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import threading
import types
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent / "hooks"))
import hook_deploy


class HookDeploySourceConsistencyTests(unittest.TestCase):
    def test_default_remote_bundle_uses_one_immutable_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "missing" / hook_deploy.CANONICAL_HOOK_NAME
            destination = root / "installed" / hook_deploy.CANONICAL_HOOK_NAME
            sha = "a" * 40
            requests = []

            def fetch(url, timeout):
                requests.append(url)
                if url == "https://api.github.com/repos/alex09x/prod-code/commits/main":
                    return io.BytesIO(json.dumps({"sha": sha}).encode())
                return io.BytesIO(b"guard component")

            with mock.patch.object(
                hook_deploy.urllib.request, "urlopen", side_effect=fetch
            ):
                hook_deploy.deploy_canonical_script(source, str(destination))
            self.assertEqual(
                requests[0],
                "https://api.github.com/repos/alex09x/prod-code/commits/main",
            )
            self.assertEqual(
                len(requests), len(hook_deploy.REQUIRED_GUARD_COMPONENTS) + 1
            )
            self.assertTrue(
                all(f"/{sha}/scripts/hooks/" in url for url in requests[1:])
            )

    def test_installer_ignores_untrusted_current_directory_before_imports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "argparse.py").write_text(
                'raise RuntimeError("untrusted import")\n', encoding="utf-8"
            )
            env = dict(os.environ)
            env["PYTHONPATH"] = str(root)
            installer = Path(__file__).parent / "hooks" / "install-hooks.py"
            result = subprocess.run(
                [sys.executable, str(installer.resolve()), "--help"],
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def _bundle(self, root, entry, package="pass\n"):
        source = root / "source"
        destination = root / "installed" / hook_deploy.CANONICAL_HOOK_NAME
        for relative in hook_deploy.REQUIRED_GUARD_COMPONENTS:
            target = source / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            text = entry if relative == hook_deploy.CANONICAL_HOOK_NAME else "pass\n"
            if relative == "prod_code_guard/__init__.py":
                text = package
            target.write_text(text, encoding="utf-8")
        hook_deploy.deploy_canonical_script(
            source / hook_deploy.CANONICAL_HOOK_NAME, str(destination)
        )
        return destination

    def test_imported_launcher_preserves_original_exception_and_sys_path(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = self._bundle(
                Path(directory), 'raise ValueError("original failure")\n'
            )
            original_path = list(sys.path)
            spec = importlib.util.spec_from_file_location(
                "test_launch_failure", destination
            )
            module = importlib.util.module_from_spec(spec)
            try:
                with self.assertRaisesRegex(ValueError, "original failure"):
                    spec.loader.exec_module(module)
                self.assertEqual(sys.path, original_path)
            finally:
                sys.path[:] = original_path

    def test_imported_launcher_uses_active_bundle_instead_of_cached_package(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = self._bundle(
                Path(directory),
                'import sys\nfrom prod_code_guard import marker\nassert marker == "fresh"\n',
                'marker = "fresh"\n',
            )
            previous = {
                name: value
                for name, value in sys.modules.items()
                if name == "prod_code_guard" or name.startswith("prod_code_guard.")
            }
            fake = types.ModuleType("prod_code_guard")
            fake.marker = "stale"
            sys.modules["prod_code_guard"] = fake
            original_path = list(sys.path)
            spec = importlib.util.spec_from_file_location(
                "test_launch_cache", destination
            )
            module = importlib.util.module_from_spec(spec)
            try:
                spec.loader.exec_module(module)
            finally:
                sys.path[:] = original_path
                for name in list(sys.modules):
                    if name == "prod_code_guard" or name.startswith("prod_code_guard."):
                        del sys.modules[name]
                sys.modules.update(previous)

    def test_incomplete_local_bundle_never_downloads_other_components(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source" / hook_deploy.CANONICAL_HOOK_NAME
            destination = root / "installed" / hook_deploy.CANONICAL_HOOK_NAME
            source.parent.mkdir()
            destination.parent.mkdir()
            source.write_text("new entrypoint", encoding="utf-8")
            destination.write_text("old entrypoint", encoding="utf-8")
            with mock.patch.object(hook_deploy.urllib.request, "urlopen") as download:
                download.side_effect = OSError("unexpected download")
                with self.assertRaises(FileNotFoundError):
                    hook_deploy.deploy_canonical_script(source, str(destination))
                download.assert_not_called()
            self.assertEqual(destination.read_text(encoding="utf-8"), "old entrypoint")

    def test_old_bundle_remains_available_to_an_inflight_hook(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            destination = root / "installed" / hook_deploy.CANONICAL_HOOK_NAME
            for relative in hook_deploy.REQUIRED_GUARD_COMPONENTS:
                target = source / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("guard component", encoding="utf-8")
            entrypoint = source / hook_deploy.CANONICAL_HOOK_NAME
            hook_deploy.deploy_canonical_script(entrypoint, str(destination))
            pinned = hook_deploy.resolve_active_version_dir(destination)
            self.assertIsNotNone(pinned)
            os.utime(pinned, (1, 1))
            for _ in range(4):
                hook_deploy.deploy_canonical_script(entrypoint, str(destination))
            self.assertTrue(
                pinned.is_dir(), "a hook may still be using its pinned version"
            )

    def test_concurrent_legacy_upgrade_failure_preserves_successful_upgrade(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            destination = root / "installed" / hook_deploy.CANONICAL_HOOK_NAME
            destination.parent.mkdir()
            destination.write_text("legacy", encoding="utf-8")
            for relative in hook_deploy.REQUIRED_GUARD_COMPONENTS:
                target = source / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("guard component", encoding="utf-8")
            entrypoint = source / hook_deploy.CANONICAL_HOOK_NAME
            first_stage = threading.Event()
            second_started = threading.Event()
            second_stage = threading.Event()
            first_done = threading.Event()
            second_saw_old = []
            errors = []
            original_stage = hook_deploy._stage_bundle
            original_replace = os.replace

            def stage(*args, **kwargs):
                if threading.current_thread().name == "first":
                    first_stage.set()
                    self.assertTrue(second_started.wait(timeout=5))
                    second_stage.wait(timeout=0.5)
                else:
                    second_saw_old.append(not first_done.is_set())
                    second_stage.set()
                result = original_stage(*args, **kwargs)
                if threading.current_thread().name == "second" and second_saw_old[0]:
                    self.assertTrue(first_done.wait(timeout=5))
                return result

            def replace(src, dst):
                if threading.current_thread().name == "second":
                    if second_saw_old[0] and Path(dst) == destination:
                        raise OSError("simulated activation failure")
                    if not second_saw_old[0] and ".current.tmp." in str(src):
                        raise OSError("simulated activation failure")
                result = original_replace(src, dst)
                if (
                    threading.current_thread().name == "first"
                    and Path(dst) == destination
                ):
                    first_done.set()
                return result

            def install():
                try:
                    if threading.current_thread().name == "second":
                        self.assertTrue(first_stage.wait(timeout=5))
                        second_started.set()
                    hook_deploy.deploy_canonical_script(entrypoint, str(destination))
                except (OSError, AssertionError, threading.BrokenBarrierError) as error:
                    errors.append(error)

            with (
                mock.patch.object(hook_deploy, "_stage_bundle", side_effect=stage),
                mock.patch.object(hook_deploy.os, "replace", side_effect=replace),
            ):
                threads = [
                    threading.Thread(target=install, name=name)
                    for name in ("first", "second")
                ]
                for thread in threads:
                    thread.start()
                for thread in threads:
                    thread.join(timeout=10)
            self.assertTrue(all(not thread.is_alive() for thread in threads))
            self.assertEqual(len(errors), 1)
            self.assertIn("simulated activation failure", str(errors[0]))
            ok, message = hook_deploy.check_canonical_hook(str(destination))
            self.assertTrue(ok, message)
