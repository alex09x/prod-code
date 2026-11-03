# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Argument-boundary regressions for grep interception."""

from .support import GuardTestCaseBase


class GrepArgumentCasesMixin(GuardTestCaseBase):
    cwd: str

    def test_issue_1803_clustered_globs_remain_file_listing(self) -> None:
        guard = self._guard()
        for command in (
            "rg --files -ig'*release*' scripts",
            "rg --files -ug'*deploy*' scripts",
            "rg --files -Lg'*.c' scripts",
            "rg -cl --files -ig'*.py' scripts",
        ):
            with self.subTest(command=command):
                self.assertIsNone(guard.symbol_grep(command, self.cwd))
                self.assertIsNone(guard.text_grep(command, self.cwd))

    def test_issue_1803_clustered_values_preserve_content_patterns(self) -> None:
        guard = self._guard()
        for command in (
            "rg --files -cg '*.rs' my_func crates/",
            "rg -ig '*.rs' my_func crates/",
            "rg -ig --files -g '*.rs' my_func crates/",
            "rg -nFemy_func crates/",
            "rg --files -nce'my_func' crates/",
        ):
            with self.subTest(command=command):
                result = guard.symbol_grep(command, self.cwd)
                self.assertIsNotNone(result)
                self.assertEqual(result[0], "my_func")
        self.assertEqual(
            guard.text_grep("rg -ne --files crates/", self.cwd),
            ("--files", "crates/"),
        )

    def test_issue_1803_string_option_values_never_enable_listing(self) -> None:
        guard = self._guard()
        for option in (
            "-r",
            "--replace",
            "--field-match-separator",
            "--field-context-separator",
            "--context-separator",
            "-E",
            "--encoding",
            "-g",
            "--glob",
        ):
            command = f"rg {option} --files my_func crates/"
            with self.subTest(command=command):
                result = guard.symbol_grep(command, self.cwd)
                self.assertIsNotNone(result)
                self.assertEqual(result[0], "my_func")
