# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Shell extraction of inline Python snippets (`-c`, heredoc, `<<<`, `cd`, `env -C`, `bash -c`)."""

import os
import re

from .shell import shell_command_option_index
from .shell_lex import (
    _ANSI_ESC_RE,
    _ANSI_MAP,
    _REDIR_RE,
    _decode_ansi_c,
    _is_var_ref,
    _read_delim,
    _read_word,
    _scan_shell_items,
    _skip_subst,
    _tokenize_chunk,
)

__all__ = [
    "_ANSI_ESC_RE",
    "_ANSI_MAP",
    "_ASSIGN_RE",
    "_PYTHON_BIN_RE",
    "_REDIR_RE",
    "_WRAPPER_FLAGS_WITH_VAL",
    "_decode_ansi_c",
    "_extract_python_args",
    "_is_var_ref",
    "_read_delim",
    "_read_word",
    "_resolve_chdir",
    "_scan_shell_items",
    "_skip_subst",
    "_tokenize_chunk",
    "_unwrap_cmd_words",
    "iter_inline_python_snippets",
]

_PYTHON_BIN_RE = re.compile(r"^python(?:\d+(?:\.\d+)*)?$")
_ASSIGN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=.*$", re.DOTALL)
_WRAPPER_FLAGS_WITH_VAL = frozenset(
    {"-u", "-g", "-h", "-p", "-C", "-r", "-t", "-n", "-o", "-f", "-a"}
)


def _resolve_chdir(base_cwd: str | None, target: str, expanded: bool) -> str | None:
    if expanded or any(ch in target for ch in "*?["):
        return None
    exp = os.path.expanduser(target)
    if os.path.isabs(exp):
        return os.path.normpath(exp)
    return (
        os.path.normpath(os.path.join(base_cwd, exp)) if base_cwd is not None else None
    )


def _unwrap_cmd_words(words: list[tuple[str, bool]], base_cwd: str | None):
    i, cmd_cwd = 0, base_cwd
    while i < len(words):
        tok = words[i][0]
        if _ASSIGN_RE.match(tok):
            i += 1
            continue
        name = os.path.basename(tok).lower()
        if name == "env":
            i += 1
            while i < len(words):
                opt, opt_exp = words[i]
                if _ASSIGN_RE.match(opt):
                    i += 1
                elif opt == "--":
                    i += 1
                    break
                elif opt in ("-C", "--chdir") and i + 1 < len(words):
                    cmd_cwd = _resolve_chdir(cmd_cwd, words[i + 1][0], words[i + 1][1])
                    i += 2
                elif opt.startswith("--chdir="):
                    cmd_cwd = _resolve_chdir(cmd_cwd, opt.split("=", 1)[1], opt_exp)
                    i += 1
                elif opt in ("-u", "--unset", "-S", "--split-string"):
                    i += 2
                elif opt.startswith("-") and opt != "-":
                    i += 1
                else:
                    break
            continue
        if name in ("command", "exec", "nice", "time", "sudo"):
            i += 1
            while i < len(words) and words[i][0].startswith("-") and words[i][0] != "-":
                if words[i][0] == "--":
                    i += 1
                    break
                i += 2 if words[i][0] in _WRAPPER_FLAGS_WITH_VAL else 1
            continue
        break
    return i, cmd_cwd


def _extract_python_args(
    args: list[tuple[str, bool]], stdin_payload: tuple[str, bool] | None
):
    i = 0
    while i < len(args):
        tok, exp = args[i]
        if tok == "--":
            i += 1
            break
        if tok == "-" or not tok.startswith("-"):
            break
        if tok in ("-W", "-X", "--check-hash-based-pycs"):
            i += 2
            continue
        if tok.startswith(("--check-hash-based-pycs=", "-W", "-X")) and len(tok) > 2:
            i += 1
            continue
        if tok.startswith("--"):
            if tok in ("--version", "--help"):
                return None
            i += 1
            continue
        flags = tok[1:]
        for pos, ch in enumerate(flags):
            if ch in ("m", "V", "h"):
                return None
            if ch == "c":
                tail = flags[pos + 1 :]
                if tail:
                    return tail, [("-c", False), *args[i + 1 :]], exp
                return (
                    (args[i + 1][0], [("-c", False), *args[i + 2 :]], args[i + 1][1])
                    if i + 1 < len(args)
                    else None
                )
            if ch in ("W", "X"):
                i += int(pos == len(flags) - 1)
                break
        i += 1
    pos = args[i:]
    if (not pos or pos[0][0] == "-") and stdin_payload is not None:
        return (
            stdin_payload[0],
            [pos[0] if pos else ("", False), *pos[1:]],
            stdin_payload[1],
        )
    return None


def iter_inline_python_snippets(command: str, initial_cwd: str, depth: int = 0):
    """Yields (code, argv_tokens, cmd_cwd, shell_expanded) for inline Python invocations."""
    if depth >= 3:
        return
    tokens, heredocs = _scan_shell_items(command)
    effective_cwd: str | None = initial_cwd
    prev_cwd: str | None = None
    cwd_stack: list[str | None] = []
    piped_stdin: tuple[str, bool] | None = None

    for idx, (kind, val) in enumerate(tokens):
        next_is_pipe = idx + 1 < len(tokens) and tokens[idx + 1][0] == "PIPE"
        if kind == "LPAREN":
            cwd_stack.append(effective_cwd)
            piped_stdin = None
        elif kind == "RPAREN":
            effective_cwd = cwd_stack.pop() if cwd_stack else effective_cwd
            piped_stdin = None
        elif kind == "SEP":
            piped_stdin = None
        elif kind == "CMD":
            words, stdin_payload = _tokenize_chunk(val, heredocs)
            in_stdin = stdin_payload if stdin_payload is not None else piped_stdin
            piped_stdin = None
            cmd_idx, cmd_cwd = _unwrap_cmd_words(words, effective_cwd)
            if cmd_idx >= len(words):
                continue
            prog, cmd_args = os.path.basename(words[cmd_idx][0]), words[cmd_idx + 1 :]
            if prog == "ssh":
                continue
            if prog == "cd" and not next_is_pipe:
                cd_args = [a for a in cmd_args if a[0] not in ("-L", "-P")]
                if cd_args and cd_args[0][0] == "--":
                    cd_args = cd_args[1:]
                old = effective_cwd
                if not cd_args:
                    effective_cwd = os.path.normpath(os.path.expanduser("~"))
                elif cd_args[0] == ("-", False):
                    effective_cwd = prev_cwd
                else:
                    effective_cwd = _resolve_chdir(
                        effective_cwd, cd_args[0][0], cd_args[0][1]
                    )
                prev_cwd = old
            elif prog in ("pushd", "popd") and not next_is_pipe:
                effective_cwd = prev_cwd = None
            elif prog in ("bash", "sh", "zsh", "fish"):
                c_idx = shell_command_option_index(cmd_args)
                if c_idx >= 0:
                    yield from iter_inline_python_snippets(
                        cmd_args[c_idx + 1][0], cmd_cwd, depth + 1
                    )
                elif in_stdin is not None:
                    pos = [w for w, _ in cmd_args if not w.startswith("-") or w == "-"]
                    if not pos or pos == ["-"]:
                        yield from iter_inline_python_snippets(
                            in_stdin[0], cmd_cwd, depth + 1
                        )
            elif prog == "cat" and next_is_pipe and stdin_payload is not None:
                pos = [w for w, _ in cmd_args if not w.startswith("-") or w == "-"]
                if not pos or pos == ["-"]:
                    piped_stdin = stdin_payload
            elif _PYTHON_BIN_RE.fullmatch(prog):
                extracted = _extract_python_args(cmd_args, in_stdin)
                if extracted is not None:
                    yield extracted[0], extracted[1], cmd_cwd, extracted[2]
