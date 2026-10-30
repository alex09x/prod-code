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

_PYTHON_BIN_RE = re.compile(r"^python(?:\d+(?:\.\d+)*)?$")
_ASSIGN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=.*$", re.DOTALL)
_REDIR_RE = re.compile(r"^(\d*)(<<<|<<-|<<|&>>|&>|>>|>&|<&|<>|>\||<|>)")
_ANSI_ESC_RE = re.compile(r"\\(x[0-9a-fA-F]{2}|[0-7]{1,3}|['\"\\nrtabfv])")
_ANSI_MAP = {
    "'": "'",
    '"': '"',
    "\\": "\\",
    "n": "\n",
    "r": "\r",
    "t": "\t",
    "a": "\a",
    "b": "\b",
    "f": "\f",
    "v": "\v",
}
_WRAPPER_FLAGS_WITH_VAL = frozenset(
    {"-u", "-g", "-h", "-p", "-C", "-r", "-t", "-n", "-o", "-f", "-a"}
)


def _decode_ansi_c(raw: str) -> str:
    def _rep(m: re.Match) -> str:
        tok = m.group(1)
        if tok in _ANSI_MAP:
            return _ANSI_MAP[tok]
        return chr(int(tok[1:], 16)) if tok.startswith("x") else chr(int(tok, 8))

    return _ANSI_ESC_RE.sub(_rep, raw)


def _skip_subst(text: str, start: int) -> int:
    depth, j, n = 1, start + 2, len(text)
    while j < n and depth > 0:
        if text[j] == "'" and text[j - 1] != "\\":
            end_q = text.find("'", j + 1)
            j = n if end_q < 0 else end_q + 1
        elif text[j] == "\\" and j + 1 < n:
            j += 2
        else:
            depth += (text[j] == "(") - (text[j] == ")")
            j += 1
    return j


def _read_delim(text: str, j: int) -> tuple[str, bool, int]:
    n, chars, quoted = len(text), [], False
    while j < n and text[j] not in " \t\r\n;&|<>()":
        c = text[j]
        if c in ("'", '"'):
            quoted, q, j = True, c, j + 1
            while j < n and text[j] != q:
                step = 2 if (q == '"' and text[j] == "\\" and j + 1 < n) else 1
                chars.append(text[j + step - 1])
                j += step
            j += int(j < n)
        elif c == "\\" and j + 1 < n:
            quoted, j = True, j + 2
            chars.append(text[j - 1])
        else:
            chars.append(c)
            j += 1
    return "".join(chars), quoted, j


def _scan_shell_items(
    command: str,
) -> tuple[list[tuple[str, str]], dict[str, tuple[str, bool]]]:
    """Single-pass quote-aware scan producing structural items and extracted heredoc bodies."""
    items: list[tuple[str, str]] = []
    cur: list[str] = []
    heredocs: dict[str, tuple[str, bool]] = {}
    pending: list[tuple[str, str, bool, bool]] = []
    i, n, in_s, in_d = 0, len(command), False, False

    def flush():
        s = "".join(cur).strip()
        cur.clear()
        if s:
            items.append(("CMD", s))

    while i < n:
        c = command[i]
        if in_s:
            cur.append(c)
            in_s, i = (c != "'"), i + 1
        elif in_d:
            if command[i : i + 2] == "$(":
                end = _skip_subst(command, i)
                cur.append(command[i:end])
                i = end
            elif c == "\\" and i + 1 < n:
                cur.append(command[i : i + 2])
                i += 2
            else:
                in_d, i = (c != '"'), i + 1
                cur.append(c)
        elif command[i : i + 2] == "$'":
            j = i + 2
            while j < n and command[j] != "'":
                j += 2 if command[j] == "\\" and j + 1 < n else 1
            if j < n:
                cur.append(
                    "'"
                    + _decode_ansi_c(command[i + 2 : j]).replace("'", "'\"'\"'")
                    + "'"
                )
                i = j + 1
            else:
                cur.append(c)
                i += 1
        elif c in ("'", '"'):
            in_s, in_d, i = (c == "'"), (c == '"'), i + 1
            cur.append(c)
        elif command[i : i + 2] == "$(":
            end = _skip_subst(command, i)
            cur.append(command[i:end])
            i = end
        elif c == "`" and command.find("`", i + 1) >= 0:
            end = command.find("`", i + 1) + 1
            cur.append(command[i:end])
            i = end
        elif c == "\\" and i + 1 < n:
            if command[i + 1] != "\n":
                cur.append(command[i : i + 2])
            i += 2
        elif c == "#" and (i == 0 or command[i - 1] in " \t\r\n;&|()"):
            nl = command.find("\n", i)
            i = n if nl < 0 else nl
        elif command[i : i + 3] == "<<<":
            cur.append("<<<")
            i += 3
        elif command[i : i + 2] == "<<":
            strip_tabs = command[i : i + 3] == "<<-"
            j = i + (3 if strip_tabs else 2)
            while j < n and command[j] in (" ", "\t"):
                j += 1
            delim, quoted, end_d = _read_delim(command, j)
            if delim:
                ph = f"__PROD_HD_{len(heredocs)}__"
                cur.append(f" {ph} ")
                pending.append((ph, delim, strip_tabs, not quoted))
                i = end_d
            else:
                cur.append(c)
                i += 1
        elif command[i : i + 2] in ("&&", "||"):
            flush()
            items.append(("SEP", command[i : i + 2]))
            i += 2
        elif c in ";|()":
            flush()
            items.append(({"|": "PIPE", "(": "LPAREN", ")": "RPAREN"}.get(c, "SEP"), c))
            i += 1
        elif c == "\n":
            flush()
            items.append(("SEP", "\n"))
            i += 1
            for ph, delim, strip_tabs, expanded in pending:
                lines: list[str] = []
                while i < n:
                    nl = command.find("\n", i)
                    raw_l = (command[i:] if nl < 0 else command[i:nl]).rstrip("\r")
                    i = n if nl < 0 else nl + 1
                    chk = raw_l.lstrip("\t") if strip_tabs else raw_l
                    if chk == delim:
                        break
                    lines.append(chk)
                heredocs[ph] = ("\n".join(lines) + ("\n" if lines else ""), expanded)
            pending.clear()
        else:
            cur.append(c)
            i += 1
    flush()
    return items, heredocs


def _is_var_ref(chunk: str, idx: int) -> bool:
    return (
        chunk[idx] == "$"
        and idx + 1 < len(chunk)
        and (chunk[idx + 1].isalpha() or chunk[idx + 1] in "_{(")
    )


def _read_word(chunk: str, i: int) -> tuple[str, bool, int]:
    buf, exp, n = [], False, len(chunk)
    while i < n and chunk[i] not in " \t\r\n<>":
        c = chunk[i]
        if c == "'":
            end = chunk.find("'", i + 1)
            buf.append(chunk[i + 1 :] if end < 0 else chunk[i + 1 : end])
            i = n if end < 0 else end + 1
        elif c == '"':
            i += 1
            while i < n and chunk[i] != '"':
                if chunk[i : i + 2] == "$(":
                    end = _skip_subst(chunk, i)
                    buf.append(chunk[i:end])
                    exp, i = True, end
                elif chunk[i] == "\\" and i + 1 < n and chunk[i + 1] in '"\\$`\n':
                    if chunk[i + 1] != "\n":
                        buf.append(chunk[i + 1])
                    i += 2
                else:
                    exp = exp or chunk[i] == "`" or _is_var_ref(chunk, i)
                    buf.append(chunk[i])
                    i += 1
            i += int(i < n)
        elif chunk[i : i + 2] == "$(":
            end = _skip_subst(chunk, i)
            buf.append(chunk[i:end])
            exp, i = True, end
        elif c == "`":
            end = chunk.find("`", i + 1)
            buf.append(chunk[i:] if end < 0 else chunk[i : end + 1])
            exp, i = True, (n if end < 0 else end + 1)
        elif c == "\\" and i + 1 < n:
            if chunk[i + 1] != "\n":
                buf.append(chunk[i + 1])
            i += 2
        else:
            exp = exp or _is_var_ref(chunk, i)
            buf.append(c)
            i += 1
    return "".join(buf), exp, i


def _tokenize_chunk(chunk: str, heredocs: dict[str, tuple[str, bool]]):
    words: list[tuple[str, bool]] = []
    stdin_payload: tuple[str, bool] | None = None
    i, n = 0, len(chunk)
    while i < n:
        while i < n and chunk[i] in " \t\r\n":
            i += 1
        if i >= n:
            break
        redir = _REDIR_RE.match(chunk[i:])
        if redir:
            op, i = redir.group(2), i + redir.end()
            while i < n and chunk[i] in " \t":
                i += 1
            if i < n:
                target, exp, i = _read_word(chunk, i)
                if op == "<<<":
                    stdin_payload = (target, exp)
            continue
        w_val, w_exp, i = _read_word(chunk, i)
        if not w_exp and w_val in heredocs:
            stdin_payload = heredocs[w_val]
        else:
            words.append((w_val, w_exp))
    return words, stdin_payload


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
