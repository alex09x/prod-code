# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Direct `sed` source-file read detection and scoped one-file fallback check."""

import os
import re
import shlex

from .shell import SHELL_C, _is_source_code_path, split_shell_segments


def _unwrap_shell_words(words):
    """Returns the command index after assignments and common shell wrappers, plus fallback flag."""
    i = 0
    fallback = False
    assignment = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$", re.DOTALL)

    while i < len(words):
        token = words[i]
        match = assignment.match(token)
        if match:
            if match.group(1) == "PROD_CODE_SOURCE_FALLBACK":
                fallback = match.group(2) == "1"
            i += 1
            continue

        name = os.path.basename(token).lower()
        if name == "env":
            i += 1
            while i < len(words):
                option = words[i]
                match = assignment.match(option)
                if match:
                    if match.group(1) == "PROD_CODE_SOURCE_FALLBACK":
                        fallback = match.group(2) == "1"
                    i += 1
                    continue
                if option == "--":
                    i += 1
                    break
                if option in ("-i", "--ignore-environment", "-0", "--null", "-v", "--debug"):
                    i += 1
                    continue
                if option in ("-u", "--unset", "-C", "--chdir", "-S", "--split-string"):
                    i += 2
                    continue
                if option.startswith(("--unset=", "--chdir=", "--split-string=")):
                    i += 1
                    continue
                if option.startswith("-") and option != "-":
                    i += 1
                    continue
                break
            continue

        if name == "sudo":
            i += 1
            while i < len(words):
                option = words[i]
                if option == "--":
                    i += 1
                    break
                if option in (
                    "-u", "--user", "-g", "--group", "-h", "--host",
                    "-p", "--prompt", "-C", "--chdir", "-r", "--role", "-t", "--type",
                ):
                    i += 2
                    continue
                if option.startswith("-"):
                    i += 1
                    continue
                break
            continue

        if name == "nice":
            i += 1
            if i < len(words) and words[i] in ("-n", "--adjustment"):
                i += 2
            elif i < len(words) and re.fullmatch(r"-[0-9]+", words[i]):
                i += 1
            continue

        if name == "time":
            i += 1
            while i < len(words) and words[i].startswith("-"):
                i += 2 if words[i] in ("-o", "--output", "-f", "--format") else 1
            continue

        if name == "command":
            i += 1
            if i < len(words) and words[i] == "-p":
                i += 1
            continue

        if name == "exec":
            i += 1
            if i < len(words) and words[i] == "-a":
                i += 2
            continue

        return i, fallback

    return i, fallback


def _sed_source_targets(args):
    """File operands read by sed, excluding its program and in-place edits."""
    positional = []
    redirected_input = []
    explicit_script = False
    in_place = False
    options = True
    i = 0

    while i < len(args):
        arg = args[i]
        if arg in ("<<", "<<-", "<<<"):
            i += 2
            continue
        if arg.startswith(("<<", "<<-", "<<<")):
            i += 1
            continue
        if arg in ("<", "0<", "<>", "0<>"):
            if i + 1 < len(args):
                redirected_input.append(args[i + 1])
            i += 2
            continue
        if arg.startswith(("<>", "0<>")) and len(arg) > 2:
            prefix = "0<>" if arg.startswith("0<>") else "<>"
            redirected_input.append(arg[len(prefix) :])
            i += 1
            continue
        input_redirect = re.match(r"^(?:[0-9]+)?<([^<].*)$", arg)
        if input_redirect:
            redirected_input.append(input_redirect.group(1))
            i += 1
            continue
        if arg in (">", ">>", "1>", "1>>", "2>", "2>>", "&>", "&>>"):
            i += 2
            continue
        if re.match(r"^(?:[0-9]+)?(?:&>>|&>|>>|>)(.+)$", arg):
            i += 1
            continue
        if re.fullmatch(r"(?:[0-9]+)?(?:&>>|&>|>>|>)", arg):
            i += 2
            continue
        if options and arg == "--":
            options = False
            i += 1
            continue
        if options and (arg == "--in-place" or arg.startswith("--in-place=")):
            in_place = True
            i += 1
            continue
        if options and arg.startswith("-i") and not arg.startswith("--"):
            in_place = True
            i += 1
            continue
        if options and arg in ("-e", "-f", "--expression", "--file"):
            explicit_script = True
            i += 2
            continue
        if options and arg.startswith(("--expression=", "--file=")):
            explicit_script = True
            i += 1
            continue
        if options and arg == "--line-length":
            i += 2
            continue
        if options and arg.startswith("--line-length="):
            i += 1
            continue
        if options and arg.startswith("-") and arg != "-":
            consume_argument = False
            for offset, option in enumerate(arg[1:]):
                if option == "i":
                    in_place = True
                    break
                if option in ("e", "f"):
                    explicit_script = True
                    consume_argument = offset == len(arg) - 2
                    break
                if option == "l" and offset == len(arg) - 2:
                    consume_argument = True
                    break
            i += 1 + int(consume_argument)
            continue

        positional.append(arg)
        i += 1

    if in_place:
        return []
    files = positional if explicit_script else positional[1:]
    return files + redirected_input


def _sed_source_invocations(command, cwd, depth=0, inherited_fallback=False):
    """Returns each sed invocation's file operands and scoped fallback flag."""
    if depth >= 3:
        return []
    invocations = []
    for part in split_shell_segments(command):
        try:
            words = shlex.split(part)
        except ValueError:
            continue
        if not words:
            continue
        index, fallback = _unwrap_shell_words(words)
        if index >= len(words):
            continue
        fallback = fallback or inherited_fallback
        remaining = shlex.join(words[index:])
        nested = SHELL_C.match(remaining)
        if nested:
            script = nested.group("script").strip()
            if len(script) >= 2 and script[0] == script[-1] and script[0] in "'\"":
                script = script[1:-1]
            invocations.extend(_sed_source_invocations(script, cwd, depth + 1, fallback))
            continue
        if os.path.basename(words[index]).lower() not in ("sed", "gsed"):
            continue
        invocations.append((_sed_source_targets(words[index + 1 :]), fallback))
    return invocations


def sed_source_target(command, cwd, depth=0):
    """Returns the source-code file sed reads, or None for stdin/resources/edits."""
    for operands, _fallback in _sed_source_invocations(command, cwd, depth):
        for path in operands:
            if _is_source_code_path(path, cwd):
                return path
    return None


def source_sed_fallback_requested(command, cwd=None):
    """An explicit one-command opt-in for source reads when prod-code is unavailable."""
    return _source_sed_fallback_requested(command, cwd or os.getcwd(), 0)


def _source_sed_fallback_requested(command, cwd, depth):
    invocations = _sed_source_invocations(command, cwd, depth)
    source_invocations = [
        (operands, fallback)
        for operands, fallback in invocations
        if any(_is_source_code_path(path, cwd) for path in operands)
    ]
    if len(source_invocations) != 1:
        return False
    operands, fallback = source_invocations[0]
    if not fallback or len(operands) != 1:
        return False
    return not any(char in operands[0] for char in "*?[")
