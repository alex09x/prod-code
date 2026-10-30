# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Quote-aware shell scanner, ANSI-C string decoder, heredoc collector, and chunk tokenizer."""

import re

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
