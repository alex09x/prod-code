"""Regression cases for the Python source-write guard."""

# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

hook = Path(__file__).parent / "hooks" / "prod-code-local-build-guard.py"
spec = importlib.util.spec_from_file_location("guard_review", hook)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class PythonSourceGuardReviewTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "repo"
        (self.root / ".git").mkdir(parents=True)
        (self.root / "src").mkdir()
        (self.root / "src" / "lib.rs").write_text("old", encoding="utf-8")

    def test_pushd_unknown_directory_does_not_block_scratch_write(self):
        cmd = "pushd /tmp && python3 -c \"open('scratch.py', 'w').write('x')\""
        self.assertIsNone(guard.scripted_source_write_target(cmd, str(self.root)))

    def test_shell_script_argument_c_is_not_a_shell_command(self):
        cmd = (
            'bash scripts/runner.sh -c "python3 -c \'open(\\"src/lib.rs\\",\\"w\\")\'"'
        )
        self.assertIsNone(guard.scripted_source_write_target(cmd, str(self.root)))

    def test_uri_is_not_a_local_source_path(self):
        cmd = "python3 -c \"open('https://example.com/source.py', 'w')\""
        self.assertIsNone(guard.scripted_source_write_target(cmd, str(self.root)))

    def test_non_object_payloads_are_ignored(self):
        for payload in ([], None, 123):
            with (
                self.subTest(payload=payload),
                mock.patch("sys.stdin", io.StringIO(json.dumps(payload))),
            ):
                self.assertEqual(guard.main(), 0)

    def test_guard_imports_its_own_package_before_untrusted_paths(self):
        fake = Path(self.tmp.name) / "untrusted" / "prod_code_guard"
        fake.mkdir(parents=True)
        (fake / "__init__.py").write_text(
            'raise RuntimeError("wrong package")\n', encoding="utf-8"
        )
        code = (
            "import importlib.util, sys; "
            "sys.path.insert(0, sys.argv[1]); sys.path.insert(1, sys.argv[2]); "
            "s = importlib.util.spec_from_file_location('checked_guard', sys.argv[3]); "
            "m = importlib.util.module_from_spec(s); s.loader.exec_module(m)"
        )
        result = subprocess.run(
            [
                sys.executable,
                "-c",
                code,
                str(fake.parent),
                str(hook.parent.resolve()),
                str(hook.resolve()),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
