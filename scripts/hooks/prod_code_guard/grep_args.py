# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Parse search arguments without confusing option values with ripgrep modes."""

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
    "--regexp",
    "--file",
    "--glob",
    "--iglob",
    "--type",
    "--type-not",
    "--type-add",
    "--type-clear",
    "--max-count",
    "--context",
    "--after-context",
    "--before-context",
    "--include",
    "--exclude",
    "--exclude-dir",
    "--label",
    "--max-depth",
    "--maxdepth",
    "--max-filesize",
    "--path-separator",
    "--sort",
    "--sortr",
    "--pre",
    "--pre-glob",
    "--encoding",
    "--max-columns",
    "--ignore-file",
    "--replace",
    "--field-match-separator",
    "--field-context-separator",
    "--context-separator",
    "--dfa-size-limit",
    "--regex-size-limit",
    "--engine",
    "--color",
    "--colors",
    "--hyperlink-format",
    "--hostname-bin",
    "--threads",
    "--generate",
}
_RG_CONTENT_MODES = {
    "--count",
    "--count-matches",
    "--files-with-matches",
    "--files-without-match",
    "--json",
}
_RG_SHORT_VALUES = frozenset("efgtTmABCrMjdE")
_OTHER_SHORT_VALUES = frozenset("efmABCgtT")


def parse_grep_arguments(tool, args):
    """Return patterns, paths and search flags, respecting value-taking clusters."""
    patterns, rest = [], []
    recursive = tool not in ("grep", "egrep")
    fixed_strings = tool == "fgrep"
    basic = tool in ("grep", "git-grep")
    files_mode = False
    short_values = _RG_SHORT_VALUES if tool == "rg" else _OTHER_SHORT_VALUES
    i = 0
    while i < len(args):
        token = args[i]
        if token == "--":
            rest.extend(args[i + 1 :])
            break
        if token.startswith("--"):
            option, separator, value = token.partition("=")
            if tool == "rg" and option in ("--files", "--type-list") and not separator:
                files_mode = True
            elif tool == "rg" and option in _RG_CONTENT_MODES and not separator:
                files_mode = False
            elif option in GREP_OPTS_WITH_ARG:
                if not separator and i + 1 < len(args):
                    i += 1
                    value = args[i]
                    separator = " "
                if option == "--regexp" and separator:
                    patterns.append(value)
            elif option == "--recursive":
                recursive = True
            elif option == "--fixed-strings":
                fixed_strings = True
            elif option in ("--extended-regexp", "--perl-regexp"):
                basic = False
            i += 1
            continue
        if token.startswith("-") and len(token) > 1:
            for position, flag in enumerate(token[1:], 1):
                if flag in short_values:
                    value = token[position + 1 :]
                    if value:
                        value = value.removeprefix("=")
                    elif i + 1 < len(args):
                        i += 1
                        value = args[i]
                    else:
                        value = None
                    if flag == "e" and value is not None:
                        patterns.append(value)
                    # The remaining characters are the value, not more options.
                    break
                if tool == "rg" and flag in "cl":
                    files_mode = False
                if tool != "rg" and flag in "rR":
                    recursive = True
                if flag == "F":
                    fixed_strings = True
                if flag in "EP":
                    basic = False
            i += 1
            continue
        rest.append(token)
        i += 1
    return patterns, rest, recursive, fixed_strings, basic, files_mode
