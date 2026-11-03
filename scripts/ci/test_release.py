#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Test release trust boundaries without any credentials or Apple requests."""

import hashlib
import io
import json
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from plan import release_input, run_tests, test_matrix
from publish import EXPECTED, collect


class ReleaseBoundaries(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def git(self, *args):
        return subprocess.check_output(
            ["git", *args], cwd=self.root, text=True, stderr=subprocess.PIPE
        ).strip()

    def repository(self):
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.com")
        (self.root / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "1.2.3"\n'
        )
        self.git("add", "Cargo.toml")
        self.git("commit", "-m", "fixture")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("tag", "v1.2.3")

    def test_release_tag_must_match_approved_main_commit_and_version(self):
        self.repository()
        self.assertEqual(
            release_input(self.root, "v1.2.3")["commit"], self.git("rev-parse", "HEAD")
        )
        self.git("tag", "v9.9.9")
        with self.assertRaisesRegex(ValueError, "version"):
            release_input(self.root, "v9.9.9")
        self.git("checkout", "-b", "unapproved")
        (self.root / "unapproved").write_text("no")
        self.git("add", ".")
        self.git("commit", "-m", "unapproved")
        self.git("tag", "-f", "v1.2.3")
        with self.assertRaises(subprocess.CalledProcessError):
            release_input(self.root, "v1.2.3")

    def test_untrusted_tag_arguments_are_rejected_before_git(self):
        for tag in ["--help", "v1.2.3;echo nope", "main", "v1.2.3\n"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release_input(self.root, tag)

    def test_a_failed_target_does_not_hide_later_tests_in_its_shard(self):
        row = {"package": "demo", "kind": "test", "targets": ["first", "second"]}
        results = [
            subprocess.CompletedProcess([], 101),
            subprocess.CompletedProcess([], 0),
        ]
        with patch("plan.subprocess.run", side_effect=results) as run:
            self.assertEqual(run_tests(self.root, row), ["first"])
        self.assertEqual(run.call_count, 2)
        self.assertEqual(run.call_args_list[1].args[0][-2:], ["--test", "second"])

    def fixture_archive(self, extra=None, tamper=False, status="Accepted"):
        incoming = self.root / "incoming"
        incoming.mkdir()
        directory = incoming / "all"
        directory.mkdir()
        mac = {
            n: hashlib.sha256(n.encode()).hexdigest()
            for n in EXPECTED
            if n.endswith("apple-darwin")
        }
        receipt = json.dumps({"status": status, "sha256": mac}).encode()
        with tarfile.open(directory / "assets.tar", "w") as archive:
            for name in sorted(EXPECTED):
                body = (
                    b"tampered"
                    if tamper and name.endswith("apple-darwin")
                    else name.encode()
                )
                item = tarfile.TarInfo("./" + name)
                item.size = len(body)
                archive.addfile(item, io.BytesIO(body))
            item = tarfile.TarInfo("./notarization.json")
            item.size = len(receipt)
            archive.addfile(item, io.BytesIO(receipt))
            if extra:
                archive.addfile(extra)
        return incoming

    def test_only_exact_six_assets_with_matching_notarization_are_collected(self):
        manifest = collect(self.fixture_archive(), self.root / "release")
        self.assertEqual(len(manifest.splitlines()), 6)
        self.assertEqual(
            {p.name for p in (self.root / "release").iterdir()},
            EXPECTED | {"SHA256SUMS"},
        )

    def test_tampered_notarized_bytes_are_refused(self):
        with self.assertRaisesRegex(ValueError, "differs"):
            collect(self.fixture_archive(tamper=True), self.root / "release")

    def test_nonaccepted_notarization_is_refused(self):
        with self.assertRaisesRegex(ValueError, "not accepted"):
            collect(self.fixture_archive(status="Invalid"), self.root / "release")

    def test_archive_path_escape_is_refused(self):
        with self.assertRaisesRegex(ValueError, "unsafe"):
            collect(
                self.fixture_archive(tarfile.TarInfo("../escape")),
                self.root / "release",
            )
        self.assertFalse((self.root / "escape").exists())

    def test_archive_symlink_is_refused(self):
        extra = tarfile.TarInfo("link")
        extra.type = tarfile.SYMTYPE
        extra.linkname = "/tmp/escape"
        with self.assertRaisesRegex(ValueError, "unsafe"):
            collect(self.fixture_archive(extra), self.root / "release")

    def test_test_matrix_covers_every_target_once_and_bounds_linked_binaries(self):
        targets = [{"name": "demo", "kind": ["lib"]}]
        targets += [{"name": f"case{i:02}", "kind": ["test"]} for i in range(19)]
        metadata = {
            "workspace_members": ["demo"],
            "packages": [{"id": "demo", "name": "demo", "targets": targets}],
        }
        rows = test_matrix(metadata)["include"]
        integration = [r for r in rows if r["kind"] == "test"]
        self.assertEqual(
            [t for r in integration for t in r["targets"]],
            [t["name"] for t in targets[1:]],
        )
        self.assertTrue(all(len(r["targets"]) <= 8 for r in rows))
        self.assertEqual([r["kind"] for r in rows[:2]], ["lib", "doc"])


if __name__ == "__main__":
    unittest.main()
