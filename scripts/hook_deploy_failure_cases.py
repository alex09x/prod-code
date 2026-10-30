# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Stepwise failure-injection and fallback-download rollback test cases."""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path
from unittest import mock

try:
    from scripts.installer_test_support import (
        InstallerTestSupportMixin,
        _FakeHttpResponse,
        _write_version_bundle,
        installer,
    )
except ModuleNotFoundError:  # pragma: no cover
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from scripts.installer_test_support import (
        InstallerTestSupportMixin,
        _FakeHttpResponse,
        _write_version_bundle,
        installer,
    )


class HookDeployFailureCasesMixin(InstallerTestSupportMixin):
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
