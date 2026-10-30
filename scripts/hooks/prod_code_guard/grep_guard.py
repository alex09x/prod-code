# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Grep/ripgrep symbol and text search interception and transcript policy checks."""

import os
import re
import shlex

from .grep_patterns import (
    DEFINITION,
    IDENT,
    RESOURCE_EXTENSIONS,
    SPECIAL_FILENAMES,
    bre_to_ripgrep,
    is_filename_or_fixture_pattern,
    looks_like_symbol,
    symbol_in,
)
from .shell import (
    CODE_EXT,
    NOT_CODE_EXT,
    first_command_in_pipeline,
    in_git_repo,
    split_pipelines,
)

__all__ = [
    "DEFINITION",
    "GREP_OPTS_WITH_ARG",
    "IDENT",
    "RESOURCE_EXTENSIONS",
    "SPECIAL_FILENAMES",
    "asked_prod_code",
    "bre_to_ripgrep",
    "is_filename_or_fixture_pattern",
    "looks_like_symbol",
    "reported_prod_code_issue",
    "ripgrep_pattern",
    "symbol_grep",
    "symbol_in",
    "text_grep",
]

GREP_OPTS_WITH_ARG = {
    "-e",
    "-f",
    "-A",
    "-B",
    "-C",
    "-m",
    "-g",
    "-t",
    "-T",
    "--glob",
    "--type",
    "--type-not",
    "--regexp",
    "--file",
    "--max-count",
    "--context",
    "--after-context",
    "--before-context",
    "--include",
    "--exclude",
}


def ripgrep_pattern(command, cwd, pat):
    """`pat`, from a grep in `command`, written the way `prod-code grep` reads it (#1581)."""
    for patterns, _paths, _recursive, fixed_strings, basic in _iter_grep_invocations(
        command, cwd
    ):
        if pat in patterns:
            if fixed_strings:
                return re.sub(r"([\\.+*?()|\[\]{}^$])", r"\\\1", pat)
            return bre_to_ripgrep(pat) if basic else pat
    return pat


def _iter_grep_invocations(command, cwd):
    """Yields (patterns, paths, recursive, fixed_strings, basic) for grep/rg/git-grep over source files."""
    preceded_by_script = False
    for pipeline in split_pipelines(command):
        if "<<heredoc" in pipeline:
            preceded_by_script = True
        first = first_command_in_pipeline(pipeline)
        try:
            words = shlex.split(first)
        except ValueError:
            continue
        while words and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", words[0]):
            words = words[1:]
        if not words:
            continue
        tool = os.path.basename(words[0])
        args = words[1:]
        if tool in (
            "python",
            "python3",
            "node",
            "ruby",
            "perl",
            "sed",
            "awk",
            "tee",
            "cat",
            "patch",
        ):
            preceded_by_script = True
            continue
        if tool in ("bash", "sh", "zsh", "fish"):
            for j, a in enumerate(args[:-1]):
                if re.fullmatch(r"-[a-z]*c", a):
                    yield from _iter_grep_invocations(args[j + 1], cwd)
            continue
        if tool == "git" and args[:1] == ["grep"]:
            tool, args = "git-grep", args[1:]
        if tool not in ("grep", "egrep", "fgrep", "rg", "ag", "git-grep"):
            continue
        recursive = tool != "grep" and tool != "egrep"
        fixed_strings = tool == "fgrep"
        basic = tool in ("grep", "git-grep")
        patterns, rest, i = [], [], 0
        while i < len(args):
            a = args[i]
            if a in ("-e", "--regexp") and i + 1 < len(args):
                patterns.append(args[i + 1])
                i += 2
                continue
            if a in GREP_OPTS_WITH_ARG and i + 1 < len(args):
                i += 2
                continue
            if a == "--":
                rest.extend(args[i + 1 :])
                break
            if a.startswith("-") and len(a) > 1:
                if re.search(r"[rR]", a) or a == "--recursive":
                    recursive = True
                if "F" in a or a == "--fixed-strings":
                    fixed_strings = True
                if a in ("--extended-regexp", "--perl-regexp") or (
                    not a.startswith("--") and re.search(r"[EP]", a)
                ):
                    basic = False
                i += 1
                continue
            rest.append(a)
            i += 1
        if not patterns and rest:
            patterns, rest = [rest[0]], rest[1:]
        if not patterns:
            continue
        paths = rest
        # A single-file non-recursive grep immediately following an inline script or heredoc
        # is a post-edit verification check on the modified file, not codebase discovery (#1125).
        if (
            preceded_by_script
            and not recursive
            and paths
            and all(
                not os.path.isdir(os.path.join(cwd or ".", os.path.expanduser(x)))
                for x in paths
            )
        ):
            continue
        if paths:
            code = [
                x
                for x in paths
                if CODE_EXT.search(x)
                or (
                    os.path.isdir(os.path.join(cwd or ".", os.path.expanduser(x)))
                    and not re.search(
                        r"(^|/)(\.claude|node_modules|target|\.build|logs?|tmp)(/|$)", x
                    )
                )
            ]
            if not code or all(NOT_CODE_EXT.search(x) for x in paths):
                continue
            if not recursive and not any(CODE_EXT.search(x) for x in paths):
                continue
        elif not recursive:
            continue
        if not in_git_repo(cwd):
            continue
        yield patterns, paths, recursive, fixed_strings, basic


def symbol_grep(command, cwd):
    """(symbol, pattern) when the command looks a symbol up over source files with grep/rg."""
    if "PROD_CODE_GREP=1" in command:
        return None
    for patterns, _paths, _recursive, fixed_strings, _basic in _iter_grep_invocations(
        command, cwd
    ):
        for pat in patterns:
            if is_filename_or_fixture_pattern(pat, cwd, fixed_strings):
                continue
            sym = symbol_in(pat)
            if sym:
                return sym, pat
    return None


def text_grep(command, cwd):
    """(pattern, target_path) when the command runs a literal, regex, or prefix text search
    over source files with grep/rg that should be routed to `code_grep` / `prod-code grep` (#1119)."""
    if "PROD_CODE_GREP=1" in command:
        return None
    for patterns, paths, _recursive, fixed_strings, _basic in _iter_grep_invocations(
        command, cwd
    ):
        target_path = None
        for p in paths:
            if (
                p != "."
                and not p.startswith("*.")
                and not re.fullmatch(r"\.[a-z0-9]+", p, re.IGNORECASE)
            ):
                target_path = p
                break
        for pat in patterns:
            if is_filename_or_fixture_pattern(pat, cwd, fixed_strings):
                continue
            if symbol_in(pat) is not None:
                continue
            return pat, target_path
    return None


def _read_transcript_tail(transcript):
    with open(transcript, "rb") as f:
        f.seek(0, 2)
        f.seek(max(0, f.tell() - 2_000_000))
        return f.read().decode("utf-8", "ignore")


def asked_prod_code(transcript, symbol, agent=None):
    """Whether this session already asked prod-code about `symbol`."""
    if not transcript or not os.path.isfile(transcript):
        return agent is None
    bare = symbol.split("::")[-1].split(".")[-1]
    try:
        tail = _read_transcript_tail(transcript)
    except OSError:
        return agent is None
    query_re = re.compile(
        r"prod-code__code_|prod_code__code_|"
        r"\"code_(definition|references|symbols|search|grep|hover|callers|slice|outline)\"|"
        r"prod-code\s+(def|refs|symbols|search|grep|hover|callers|slice|outline)\b"
    )
    return any(bare in line and query_re.search(line) for line in tail.splitlines())


def reported_prod_code_issue(transcript, symbol, agent=None):
    """Whether this session filed an issue about prod-code via code_report_issue or CLI."""
    if not transcript or not os.path.isfile(transcript):
        return agent is None
    try:
        tail = _read_transcript_tail(transcript)
    except OSError:
        return agent is None
    report_re = re.compile(
        r"prod-code__code_report_issue|prod_code__code_report_issue|"
        r"\"code_report_issue\"|prod-code\s+report-issue\b"
    )
    return any(report_re.search(line) for line in tail.splitlines())
