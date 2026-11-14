#!/usr/bin/env python3
#
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
#

"""Tests for rustup-unowned-files.sh on toolchain directories laid out the way rustup does."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("rustup-unowned-files.sh")
TARGET = "x86_64-unknown-linux-gnu"


def write(root: Path, relative: str, text: str = "") -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def toolchain(home: Path, name: str, components: dict[str, list[str]]) -> Path:
    """A toolchain whose installed components own the given `file:` and `dir:` entries."""
    root = home / "toolchains" / name
    write(root, "lib/rustlib/components", "".join(f"{c}\n" for c in components))
    write(root, "lib/rustlib/multirust-config.toml", "config_version = \"1\"\n")
    for component, entries in components.items():
        write(root, f"lib/rustlib/manifest-{component}", "".join(f"{e}\n" for e in entries))
        for entry in entries:
            kind, relative = entry.split(":", 1)
            write(root, relative if kind == "file" else f"{relative}/index.html")
    return root


def report(home: Path, *args: str) -> str:
    done = subprocess.run(
        ["bash", str(SCRIPT), *args],
        env={**os.environ, "RUSTUP_HOME": str(home)},
        capture_output=True,
        text=True,
        check=True,
    )
    return done.stdout


class UnownedFilesTests(unittest.TestCase):
    def test_a_file_left_by_a_forgotten_component_is_listed_and_owned_ones_are_not(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            clean = toolchain(
                home,
                f"1.97.1-{TARGET}",
                {
                    f"rustc-{TARGET}": ["file:bin/rustc", "file:lib/libLLVM.so"],
                    f"llvm-tools-preview-{TARGET}": [
                        f"file:lib/rustlib/{TARGET}/lib/libLLVM.so",
                        f"file:lib/rustlib/{TARGET}/bin/llvm-cov",
                    ],
                    f"rust-docs-{TARGET}": ["dir:share/doc/rust/html"],
                },
            )
            # A file inside a directory a component owns as a whole is owned.
            write(clean, "share/doc/rust/html/std/index.html")
            broken = toolchain(
                home,
                f"1.98.1-{TARGET}",
                {f"rustc-{TARGET}": ["file:bin/rustc", "file:lib/libLLVM.so"]},
            )
            # What an install rustup lost track of leaves behind: the file, and no record.
            write(broken, f"lib/rustlib/{TARGET}/lib/libLLVM.so")

            out = report(home)

            self.assertIn(f"1.97.1-{TARGET}: 0 file(s) no installed component owns", out)
            self.assertIn(
                f"1.98.1-{TARGET}: 1 file(s) no installed component owns\n"
                f"    lib/rustlib/{TARGET}/lib/libLLVM.so\n",
                out,
            )

    def test_a_recorded_component_without_its_manifest_is_named(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            root = toolchain(home, f"stable-{TARGET}", {f"cargo-{TARGET}": ["file:bin/cargo"]})
            (root / f"lib/rustlib/manifest-cargo-{TARGET}").unlink()

            out = report(home)

            self.assertIn(f"stable-{TARGET}: component cargo-{TARGET} has no manifest", out)
            self.assertIn("1 file(s) no installed component owns\n    bin/cargo\n", out)

    def test_a_long_list_is_cut_unless_all_is_asked_for(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            root = toolchain(home, f"nightly-{TARGET}", {f"rustc-{TARGET}": ["file:bin/rustc"]})
            for index in range(25):
                write(root, f"stray/file-{index:02}")

            cut = report(home)
            self.assertIn("25 file(s) no installed component owns", cut)
            self.assertIn("    stray/file-19\n    ... and 5 more (--all lists them)\n", cut)
            self.assertNotIn("stray/file-20", cut)

            self.assertIn("    stray/file-24\n", report(home, "--all"))


if __name__ == "__main__":
    unittest.main()
