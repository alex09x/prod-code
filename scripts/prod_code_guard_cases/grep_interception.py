# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Grep symbol-interception and shell-payload parsing test cases for the guard hook."""

from __future__ import annotations

import os
import tempfile

from .grep_arguments import GrepArgumentCasesMixin


class GrepInterceptionCasesMixin(GrepArgumentCasesMixin):
    temp_dir: tempfile.TemporaryDirectory[str] | None = None
    cwd: str = ""

    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.cwd = self.temp_dir.name
        # Initialize a mock git directory with source code folders
        os.makedirs(os.path.join(self.cwd, ".git"), exist_ok=True)
        os.makedirs(os.path.join(self.cwd, "crates"), exist_ok=True)
        os.makedirs(os.path.join(self.cwd, "src"), exist_ok=True)
        os.makedirs(os.path.join(self.cwd, "scripts"), exist_ok=True)

    def tearDown(self) -> None:
        if self.temp_dir is not None:
            self.temp_dir.cleanup()

    def test_fixture_filename_search_allowed_issue_765(self) -> None:
        guard = self._guard()
        # Issue #765 reproduction: git grep -n -F -e meta.json -- ".rs" ".md"
        cmd = 'git grep -n -F -e meta.json -- ".rs" ".md"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Literal fixture search for meta.json with -F must be allowed (#765)",
        )

    def test_textual_filename_searches_allowed(self) -> None:
        guard = self._guard()
        allowed_searches = [
            'git grep -n "meta.json"',
            'git grep -n "Cargo.lock"',
            'git grep -n "config.toml"',
            'git grep -n -F "fixtures/meta.json"',
            'git grep -n "TODO"',
            'git grep -n "error: unexpected end of file"',
        ]
        for cmd in allowed_searches:
            with self.subTest(command=cmd):
                self.assertIsNone(
                    guard.symbol_grep(cmd, self.cwd),
                    f"Text search `{cmd}` should be allowed without symbol interception",
                )

    def test_bounded_search_in_an_explicit_manifest_is_allowed(self) -> None:
        with open(os.path.join(self.cwd, "Cargo.toml"), "w", encoding="utf-8") as manifest:
            manifest.write("[profile.dev]\nopt-level = 1\n")
        guard = self._guard()
        command = "rg -n -A12 '^\\[profile' Cargo.toml"
        self.assertIsNone(guard.symbol_grep(command, self.cwd))
        self.assertIsNone(guard.text_grep(command, self.cwd))
        payload = {
            "toolCall": {
                "name": "run_command",
                "args": {"CommandLine": command, "Cwd": self.cwd},
            },
            "workspacePaths": [self.cwd],
        }
        self.assertEqual(self._invoke_main_stdio(payload), "")

    def test_fixed_string_workspace_fixture_allowed(self) -> None:
        guard = self._guard()
        fixtures_dir = os.path.join(self.cwd, "tests", "fixtures")
        os.makedirs(fixtures_dir, exist_ok=True)
        fixture_path = os.path.join(fixtures_dir, "custom_fixture_raw")
        with open(fixture_path, "w") as f:
            f.write("test data\n")

        cmd = 'git grep -n -F "custom_fixture_raw" -- "*.rs"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Fixed-string search naming an existing workspace fixture should be allowed",
        )

    def test_identifier_searches_denied(self) -> None:
        guard = self._guard()
        denied_searches = [
            ('git grep -n "my_func" crates/', "my_func"),
            ('git grep -n "fn file_id_for_path" crates/', "file_id_for_path"),
            ('git grep -n "Type::method" src/', "Type::method"),
            ('git grep -n "pkg.Func" .', "pkg.Func"),
            ('git grep -n -F "my_func" crates/', "my_func"),
        ]
        for cmd, expected_symbol in denied_searches:
            with self.subTest(command=cmd):
                res = guard.symbol_grep(cmd, self.cwd)
                self.assertIsNotNone(res, f"Grep command `{cmd}` should be intercepted")
                self.assertEqual(res[0], expected_symbol)

    def test_multiple_patterns_allowed(self) -> None:
        guard = self._guard()
        cmd = 'git grep -n -F -e meta.json -e config.toml -e README.md -- ".rs"'
        self.assertIsNone(
            guard.symbol_grep(cmd, self.cwd),
            "Multiple fixture/filename search patterns should all be allowed",
        )

    def test_case_insensitive_extensions_allowed(self) -> None:
        guard = self._guard()
        cmds = [
            'git grep -n "meta.JSON"',
            'git grep -n "Cargo.LOCK"',
            'git grep -n -F "fixture.YAML"',
        ]
        for cmd in cmds:
            with self.subTest(command=cmd):
                self.assertIsNone(
                    guard.symbol_grep(cmd, self.cwd),
                    f"{cmd} with uppercase extension should be allowed",
                )

    def test_antigravity_and_claude_payload_parsing(self) -> None:
        guard = self._guard()
        # Antigravity format
        agy_payload = {
            "toolCall": {
                "name": "run_command",
                "args": {
                    "CommandLine": 'git grep -n -F -e meta.json -- ".rs"',
                    "Cwd": self.cwd,
                },
            }
        }
        self.assertEqual(
            guard.shell_command(agy_payload),
            'git grep -n -F -e meta.json -- ".rs"',
        )

        # Claude Code format
        claude_payload = {
            "tool_input": {
                "command": 'git grep -n -F -e meta.json -- ".rs"',
            }
        }
        self.assertEqual(
            guard.shell_command(claude_payload),
            'git grep -n -F -e meta.json -- ".rs"',
        )

    def test_ripgrep_files_listing_allowed_issue_1803(self) -> None:
        guard = self._guard()
        # Issue #1803: bounded filename enumeration with --files must not be blocked as content search
        cmd_exact = "rg --files scripts -g '*release*' -g '*deploy*' -g '*rollout*'"
        self.assertIsNone(guard.symbol_grep(cmd_exact, self.cwd))
        self.assertIsNone(guard.text_grep(cmd_exact, self.cwd))

        payload = {
            "toolCall": {
                "name": "run_command",
                "args": {"CommandLine": cmd_exact, "Cwd": self.cwd},
            },
            "workspacePaths": [self.cwd],
        }
        self.assertEqual(self._invoke_main_stdio(payload), "")

        allowed_files_cases = [
            "rg scripts --files -g '*release*'",
            "rg -g '*release*' --files scripts",
            "rg --files scripts",
            "rg --files",
            "rg --files .",
            "rg --files -- scripts",
            "rg --files --glob='*release*' scripts",
            "rg --files --iglob='*release*' scripts",
            "rg --files --type=py scripts",
            "rg --files --type-not=rust scripts",
            "rg --files -g'*release*' scripts",
            "rg --files -tpy scripts",
            "rg --files -Trust scripts",
            "rg --files -t py scripts",
            "rg --files -T rust scripts",
            "rg --files scripts crates",
            "rg --count --files scripts",
            "rg -c --files scripts",
            "rg -l --files scripts",
            "rg -cl --files scripts",
            "rg -zc --files scripts",
            "rg --files-with-matches --files scripts",
            "rg --files-without-match --files scripts",
            "rg --count-matches --files scripts",
            "rg --files --count scripts --files",
            "rg --files -c scripts --files",
        ]
        for cmd in allowed_files_cases:
            with self.subTest(command=cmd):
                self.assertIsNone(
                    guard.symbol_grep(cmd, self.cwd),
                    f"Ripgrep file listing `{cmd}` should not be flagged as symbol grep",
                )
                self.assertIsNone(
                    guard.text_grep(cmd, self.cwd),
                    f"Ripgrep file listing `{cmd}` should not be flagged as text grep",
                )

    def test_ripgrep_last_mode_wins_content_modes_denied_after_files(self) -> None:
        guard = self._guard()
        # Content modes following --files switch ripgrep to content search and must be intercepted
        content_symbol_cases = [
            ("rg --files --count my_func crates/", "my_func"),
            ("rg --files --files-with-matches my_func crates/", "my_func"),
            ("rg --files --files-without-match my_func crates/", "my_func"),
            ("rg --files --count-matches my_func crates/", "my_func"),
            ("rg --files -c my_func crates/", "my_func"),
            ("rg --files -l my_func crates/", "my_func"),
            ("rg --files -cl my_func crates/", "my_func"),
            ("rg --files -zc my_func crates/", "my_func"),
            ("rg --files -zl my_func crates/", "my_func"),
            ("rg --files -ic my_func crates/", "my_func"),
        ]
        for cmd, sym in content_symbol_cases:
            with self.subTest(command=cmd):
                res = guard.symbol_grep(cmd, self.cwd)
                self.assertIsNotNone(
                    res,
                    f"Content mode after --files `{cmd}` must intercept symbol `{sym}`",
                )
                self.assertEqual(res[0], sym)
                payload = {
                    "toolCall": {
                        "name": "run_command",
                        "args": {"CommandLine": cmd, "Cwd": self.cwd},
                    },
                    "workspacePaths": [self.cwd],
                }
                out = self._invoke_main_stdio(payload)
                self.assertIn(f"Grep for code symbol `{sym}` is blocked", out)

        content_text_cases = [
            ("rg --files --count 'fn test_' crates/", "fn test_", "crates/"),
            (
                "rg --files --files-with-matches 'fn test_' crates/",
                "fn test_",
                "crates/",
            ),
            (
                "rg --files --files-without-match 'fn test_' crates/",
                "fn test_",
                "crates/",
            ),
            ("rg --files --count-matches 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --files -c 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --files -l 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --files -cl 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --files -zc 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --files -zl 'fn test_' crates/", "fn test_", "crates/"),
        ]
        for cmd, pat, path in content_text_cases:
            with self.subTest(command=cmd):
                res = guard.text_grep(cmd, self.cwd)
                self.assertIsNotNone(
                    res,
                    f"Content mode after --files `{cmd}` must intercept text pattern `{pat}`",
                )
                self.assertEqual(res, (pat, path))
                payload = {
                    "toolCall": {
                        "name": "run_command",
                        "args": {"CommandLine": cmd, "Cwd": self.cwd},
                    },
                    "workspacePaths": [self.cwd],
                }
                out = self._invoke_main_stdio(payload)
                self.assertIn(
                    f"Grep for text pattern `{pat}` over source files is blocked", out
                )

    def test_compound_command_with_files_preserves_content_denials(self) -> None:
        guard = self._guard()
        # Compound commands: --files in one segment must not allow content/identifier searches
        compound_symbol_cases = [
            ("rg --files scripts && rg my_func crates/", "my_func"),
            ("rg my_func crates/ && rg --files scripts", "my_func"),
            ("bash -c 'rg --files scripts && rg my_func crates/'", "my_func"),
            ("sh -c 'rg --files scripts; rg my_func crates/'", "my_func"),
        ]
        for cmd, sym in compound_symbol_cases:
            with self.subTest(command=cmd):
                res = guard.symbol_grep(cmd, self.cwd)
                self.assertIsNotNone(
                    res, f"Compound command `{cmd}` must intercept symbol `{sym}`"
                )
                self.assertEqual(res[0], sym)
                payload = {
                    "toolCall": {
                        "name": "run_command",
                        "args": {"CommandLine": cmd, "Cwd": self.cwd},
                    },
                    "workspacePaths": [self.cwd],
                }
                out = self._invoke_main_stdio(payload)
                self.assertIn(f"Grep for code symbol `{sym}` is blocked", out)

        compound_text_cases = [
            ("rg --files scripts && rg 'fn test_' crates/", "fn test_", "crates/"),
            ("rg 'fn test_' crates/ && rg --files scripts", "fn test_", "crates/"),
            ("rg --files scripts; rg 'fn test_' crates/", "fn test_", "crates/"),
            (
                "bash -c 'rg --files scripts && rg \"fn test_\" crates/'",
                "fn test_",
                "crates/",
            ),
        ]
        for cmd, pat, path in compound_text_cases:
            with self.subTest(command=cmd):
                res = guard.text_grep(cmd, self.cwd)
                self.assertIsNotNone(
                    res, f"Compound command `{cmd}` must intercept text pattern `{pat}`"
                )
                self.assertEqual(res, (pat, path))
                payload = {
                    "toolCall": {
                        "name": "run_command",
                        "args": {"CommandLine": cmd, "Cwd": self.cwd},
                    },
                    "workspacePaths": [self.cwd],
                }
                out = self._invoke_main_stdio(payload)
                self.assertIn(
                    f"Grep for text pattern `{pat}` over source files is blocked", out
                )

    def test_negative_content_search_examples(self) -> None:
        guard = self._guard()
        # Non-file-listing searches using literal '--files' pattern must be denied
        negative_text_cases = [
            ("rg -- --files scripts", "--files", "scripts"),
            ("rg -e --files scripts", "--files", "scripts"),
            ("rg --regexp=--files scripts", "--files", "scripts"),
            ("rg -e=--files scripts", "--files", "scripts"),
            ("rg -g --files 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --glob=--files 'fn test_' crates/", "fn test_", "crates/"),
            ("rg -g'*--files*' 'fn test_' crates/", "fn test_", "crates/"),
        ]
        for cmd, pat, path in negative_text_cases:
            with self.subTest(command=cmd):
                self.assertIsNone(guard.symbol_grep(cmd, self.cwd))
                self.assertEqual(guard.text_grep(cmd, self.cwd), (pat, path))
                payload = {
                    "toolCall": {
                        "name": "run_command",
                        "args": {"CommandLine": cmd, "Cwd": self.cwd},
                    },
                    "workspacePaths": [self.cwd],
                }
                out = self._invoke_main_stdio(payload)
                self.assertIn(
                    f"Grep for text pattern `{pat}` over source files is blocked", out
                )

        # Options with equals and attached forms for symbol searches must be denied
        symbol_option_cases = [
            ("rg --type=rust my_func crates/", "my_func"),
            ("rg -trust my_func crates/", "my_func"),
            ("rg --glob='*.rs' my_func crates/", "my_func"),
            ("rg -g'*.rs' my_func crates/", "my_func"),
            ("rg --regexp=my_func crates/", "my_func"),
            ("rg -e=my_func crates/", "my_func"),
            ("rg -emy_func crates/", "my_func"),
            ("rg -g --files my_func crates/", "my_func"),
            ("rg --glob=--files my_func crates/", "my_func"),
            ("rg -g'*--files*' my_func crates/", "my_func"),
        ]
        for cmd, sym in symbol_option_cases:
            with self.subTest(command=cmd):
                res = guard.symbol_grep(cmd, self.cwd)
                self.assertIsNotNone(res, f"`{cmd}` must intercept symbol `{sym}`")
                self.assertEqual(res[0], sym)

        # Options with equals and attached forms for text searches must be denied
        text_option_cases = [
            ("rg --type=rust 'fn test_' crates/", "fn test_", "crates/"),
            ("rg -trust 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --glob='*.rs' 'fn test_' crates/", "fn test_", "crates/"),
            ("rg -g'*.rs' 'fn test_' crates/", "fn test_", "crates/"),
            ("rg --regexp='fn test_' crates/", "fn test_", "crates/"),
            ("rg -e='fn test_' crates/", "fn test_", "crates/"),
            ("rg -e'fn test_' crates/", "fn test_", "crates/"),
        ]
        for cmd, pat, path in text_option_cases:
            with self.subTest(command=cmd):
                self.assertIsNone(guard.symbol_grep(cmd, self.cwd))
                self.assertEqual(guard.text_grep(cmd, self.cwd), (pat, path))
