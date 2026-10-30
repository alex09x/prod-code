# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Shell command segmentation, local build detection, and workspace path helpers."""

import os
import re

REASON = (
    "Run it on a build node: code_check / code_test / code_lint / code_exec "
    "(CLI: prod-code exec -- <command>). If it truly must run on this Mac, "
    "prefix it with PROD_CODE_LOCAL=1."
)

HEAVY = [
    re.compile(r"^(\S*/)?cargo(\s+\+\S+)?\s+(test|check|clippy|build|bench|llvm-cov|nextest)\b"),
    # go test / go vet stay local for now: a Go module with macOS-only cgo (the prod repo) has no
    # node to run on until a macOS node serves Go, and blocking them left its workers no way at all.
    re.compile(r"^(\S*/)?pytest\b"),
    re.compile(r"^(\S*/)?python3?\s+-m\s+pytest\b"),
    re.compile(r"^(\S*/)?(npm|pnpm|yarn|bun)\s+(run\s+)?test\b"),
    re.compile(r"^(\S*/)?xcodebuild\b.*\b(build|test)\b"),
    re.compile(r"^(\S*/)?swift\s+(build|test)\b"),
]

ALLOWED_ANYWHERE = [
    re.compile(r"PROD_CODE_LOCAL=1"),
    re.compile(r"\bprod-code\s+exec\b"),
    re.compile(r"\bprod-code\s+shadow-run\b"),
    re.compile(r"(^|[\s;&|(])ssh\s"),
]

HEREDOC_QUOTED = re.compile(
    r"<<-?\s*(?:(['\"])(?P<qdelim>[A-Za-z0-9_-]+)\1|\\(?P<edelim>[A-Za-z0-9_-]+))[^\n]*\n.*?\n\s*(?:(?P=qdelim)|(?P=edelim))\s*['\"]?\s*(?=\n|$)",
    re.DOTALL,
)
HEREDOC_UNQUOTED = re.compile(
    r"(?P<prefix>[^\n]*?)<<-?\s*(?P<delim>[A-Za-z0-9_-]+)[^\n]*\n(?P<body>.*?)\n\s*(?P=delim)\s*['\"]?\s*(?=\n|$)",
    re.DOTALL,
)
SCRIPT_INTERPRETER_PREFIX = re.compile(
    r"\b(?:python3?|node|ruby|perl|php|lua|awk|sed)\b"
)
SHELL_C = re.compile(r"^(\S*/)?(bash|sh|zsh|fish)(\s+-\w+)*\s+-l?c\s+(?P<script>.+)$", re.DOTALL)

CODE_EXT = re.compile(
    r"\.(rs|go|ts|tsx|js|jsx|mjs|cjs|py|pyi|c|cc|cpp|cxx|h|hh|hpp|hxx|swift|m|mm|kt|java|cs|rb|php|scala)$"
)
NOT_CODE_EXT = re.compile(r"\.(md|txt|log|jsonl?|ya?ml|toml|lock|csv|html?|xml|ini|cfg|conf|env|sql|sh)$")


def without_heredocs(command, extract_substs=True):
    """The command with every here-document body replaced with `<<heredoc`.
    For unquoted here-documents when `extract_substs=True` (build-command check),
    command substitutions `$(...)` and backticks are preserved unless the heredoc
    is fed directly to a non-shell script interpreter. For grep checks (`extract_substs=False`),
    heredoc bodies are always treated strictly as data."""
    cleaned = HEREDOC_QUOTED.sub("<<heredoc", command)
    substs = []

    def handle_unquoted(m):
        prefix = m.group("prefix") or ""
        body = m.group("body")
        if extract_substs and body:
            is_interpreter = bool(SCRIPT_INTERPRETER_PREFIX.search(prefix))
            pattern = r"\$\((.*?)\)" if is_interpreter else r"\$\((.*?)\)|`([^`]*)`"
            for sub in re.findall(pattern, body, re.DOTALL):
                if isinstance(sub, tuple):
                    substs.append(sub[0] or sub[1])
                elif sub:
                    substs.append(sub)
        return prefix + "<<heredoc"

    cleaned = HEREDOC_UNQUOTED.sub(handle_unquoted, cleaned).replace(r"\`", "")
    if substs:
        cleaned += "\n" + "\n".join(substs)
    return cleaned


def _consume_cmd_subst(cleaned, i, n):
    depth = 1
    j = i + 2
    while j < n and depth > 0:
        if cleaned[j] == "'" and cleaned[j - 1] != "\\":
            end_q = cleaned.find("'", j + 1)
            j = n if end_q == -1 else end_q + 1
            continue
        if cleaned[j] == "(" and cleaned[j - 1] != "\\":
            depth += 1
        elif cleaned[j] == ")" and cleaned[j - 1] != "\\":
            depth -= 1
        j += 1
    return j


def split_shell_segments(command):
    """Splits a shell command string into simple command segments, respecting single/double quotes,
    parentheses, and command substitutions."""
    cleaned = without_heredocs(command)
    segments_list = []
    cur = []
    extracted_substs = []
    i = 0
    n = len(cleaned)
    in_single = False
    in_double = False

    while i < n:
        c = cleaned[i]
        if c == "\\" and not in_single and i + 1 < n:
            cur.append(cleaned[i : i + 2])
            i += 2
            continue
        if c == "'" and not in_double:
            in_single = not in_single
            cur.append(c)
            i += 1
            continue
        if c == '"' and not in_single:
            in_double = not in_double
            cur.append(c)
            i += 1
            continue

        if not in_single:
            if cleaned[i : i + 2] == "$(":
                j = _consume_cmd_subst(cleaned, i, n)
                extracted_substs.append(cleaned[i + 2 : j - 1])
                cur.append(cleaned[i:j])
                i = j
                continue
            if c == "`":
                j = cleaned.find("`", i + 1)
                if j != -1:
                    extracted_substs.append(cleaned[i + 1 : j])
                    cur.append(cleaned[i : j + 1])
                    i = j + 1
                    continue

        if not in_single and not in_double:
            if cleaned[i : i + 2] in ("&&", "||"):
                seg = "".join(cur).strip()
                if seg:
                    segments_list.append(seg)
                cur = []
                i += 2
                continue
            if c in (";", "|", "\n", "(", ")"):
                seg = "".join(cur).strip()
                if seg:
                    segments_list.append(seg)
                cur = []
                i += 1
                continue

        cur.append(c)
        i += 1

    seg = "".join(cur).strip()
    if seg:
        segments_list.append(seg)

    for sub in extracted_substs:
        segments_list.extend(split_shell_segments(sub))

    return segments_list


def segments(command):
    """The simple commands of a shell line, each without leading VAR=value assignments."""
    for part in split_shell_segments(command):
        words = part.strip().split()
        while words and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", words[0]):
            words = words[1:]
        while words and words[0] in ("time", "exec", "nice", "env", "command", "sudo"):
            words = words[1:]
        if words:
            yield " ".join(words)


def verdict(command, depth=0):
    if any(p.search(command) for p in ALLOWED_ANYWHERE):
        return None
    for seg in segments(command):
        nested = SHELL_C.match(seg)
        if nested and depth < 3:
            script = nested.group("script").strip()
            if len(script) >= 2 and script[0] == script[-1] and script[0] in "'\"":
                script = script[1:-1]
            if verdict(script, depth + 1):
                return REASON
            continue
        if re.match(r"^(\S*/)?cargo\s+build\b", seg) and re.search(r"-p\s+prod-code-client\b", seg):
            continue
        if any(p.match(seg) for p in HEAVY):
            return REASON
    return None


def git_repo_root(path):
    d = os.path.abspath(path or ".")
    if not os.path.isdir(d):
        d = os.path.dirname(d)
    while True:
        if os.path.exists(os.path.join(d, ".git")):
            return d
        parent = os.path.dirname(d)
        if parent == d:
            return None
        d = parent


def in_git_repo(cwd):
    return git_repo_root(cwd) is not None


def split_pipelines(command):
    """Splits command into pipelines separated by &&, ||, ;, \\n outside quotes."""
    cleaned = without_heredocs(command, extract_substs=False)
    pipelines = []
    cur = []
    i = 0
    n = len(cleaned)
    in_single = False
    in_double = False

    while i < n:
        c = cleaned[i]
        if c == "\\" and not in_single and i + 1 < n:
            cur.append(cleaned[i : i + 2])
            i += 2
            continue
        if c == "'" and not in_double:
            in_single = not in_single
            cur.append(c)
            i += 1
            continue
        if c == '"' and not in_single:
            in_double = not in_double
            cur.append(c)
            i += 1
            continue

        if not in_single and not in_double:
            if cleaned[i : i + 2] in ("&&", "||"):
                pipe = "".join(cur).strip()
                if pipe:
                    pipelines.append(pipe)
                cur = []
                i += 2
                continue
            if c in (";", "\n"):
                pipe = "".join(cur).strip()
                if pipe:
                    pipelines.append(pipe)
                cur = []
                i += 1
                continue

        cur.append(c)
        i += 1

    pipe = "".join(cur).strip()
    if pipe:
        pipelines.append(pipe)
    return pipelines


def first_command_in_pipeline(pipeline):
    """Returns the first command of a pipeline (before the first pipe | outside quotes)."""
    cur = []
    i = 0
    n = len(pipeline)
    in_single = False
    in_double = False

    while i < n:
        c = pipeline[i]
        if c == "\\" and not in_single and i + 1 < n:
            cur.append(pipeline[i : i + 2])
            i += 2
            continue
        if c == "'" and not in_double:
            in_single = not in_single
            cur.append(c)
            i += 1
            continue
        if c == '"' and not in_single:
            in_double = not in_double
            cur.append(c)
            i += 1
            continue

        if not in_single and not in_double and c == "|":
            break

        cur.append(c)
        i += 1

    return "".join(cur).strip()


def _is_source_code_path(path, cwd):
    if not path or path == "-" or path in ("/dev/stdin", "/dev/fd/0"):
        return False
    expanded = os.path.expanduser(path)
    resolved = expanded if os.path.isabs(expanded) else os.path.join(cwd or os.getcwd(), expanded)
    return (
        CODE_EXT.search(resolved.lower()) is not None
        or CODE_EXT.search(os.path.realpath(resolved).lower()) is not None
    )
