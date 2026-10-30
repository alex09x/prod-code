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
"""PreToolUse hook for Bash: heavy builds, tests and lints run on the build nodes through
prod-code, not on this Mac. Denies the command with a pointer to code_check / code_test /
code_lint / code_exec. Allowed as is: anything through `prod-code exec` or over ssh (a node
does the work), the dev build of the prod-code client, and any command carrying
PROD_CODE_LOCAL=1 when it really must run here.

It also denies looking a symbol up with grep/rg inside a git repository: a pattern that is a
code identifier (snake_case, camelCase, Type::method, .Method( ) or a definition (fn/func/def/
class/struct/type Name) searched over source files. code_definition / code_references /
code_symbols answer that exactly. Text searches (plain words, phrases, regexes), searches over
logs, docs or configs, and grep as a filter on a pipe stay allowed. PROD_CODE_GREP=1 in the
command lets a symbol grep through; every such override is logged, because it marks a place
where prod-code did not serve (and should get a code_report_issue)."""
import fcntl
import json
import os
import re
import shlex
import sys
import time

LOG = os.environ.get("PROD_CODE_GUARD_LOG") or os.path.expanduser("~/.claude/hooks/prod-code-guard.jsonl")

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
    re.S,
)
HEREDOC_UNQUOTED = re.compile(
    r"(?P<prefix>[^\n]*?)<<-?\s*(?P<delim>[A-Za-z0-9_-]+)[^\n]*\n(?P<body>.*?)\n\s*(?P=delim)\s*['\"]?\s*(?=\n|$)",
    re.S,
)
SCRIPT_INTERPRETER_PREFIX = re.compile(
    r"\b(?:python3?|node|ruby|perl|php|lua|awk|sed)\b"
)


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
            for sub in re.findall(pattern, body, re.S):
                if isinstance(sub, tuple):
                    substs.append(sub[0] or sub[1])
                elif sub:
                    substs.append(sub)
        return prefix + "<<heredoc"

    cleaned = HEREDOC_UNQUOTED.sub(handle_unquoted, cleaned).replace(r"\`", "")
    if substs:
        cleaned += "\n" + "\n".join(substs)
    return cleaned


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
            cur.append(cleaned[i:i+2])
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
            if cleaned[i:i+2] == "$(":
                depth = 1
                j = i + 2
                while j < n and depth > 0:
                    if cleaned[j] == "'" and cleaned[j-1] != "\\":
                        end_q = cleaned.find("'", j + 1)
                        j = n if end_q == -1 else end_q + 1
                        continue
                    if cleaned[j] == "(" and cleaned[j-1] != "\\": depth += 1
                    elif cleaned[j] == ")" and cleaned[j-1] != "\\": depth -= 1
                    j += 1
                subcmd = cleaned[i+2:j-1]
                extracted_substs.append(subcmd)
                cur.append(cleaned[i:j])
                i = j
                continue
            if c == "`":
                j = cleaned.find("`", i + 1)
                if j != -1:
                    subcmd = cleaned[i+1:j]
                    extracted_substs.append(subcmd)
                    cur.append(cleaned[i:j+1])
                    i = j + 1
                    continue
            if cleaned[i:i+2] in ("&&", "||"):
                seg = "".join(cur).strip()
                if seg: segments_list.append(seg)
                cur = []
                i += 2
                continue
            if c in (";", "|", "\n", "(", ")"):
                seg = "".join(cur).strip()
                if seg: segments_list.append(seg)
                cur = []
                i += 1
                continue
        elif in_double:
            if cleaned[i:i+2] == "$(":
                depth = 1
                j = i + 2
                while j < n and depth > 0:
                    if cleaned[j] == "'" and cleaned[j-1] != "\\":
                        end_q = cleaned.find("'", j + 1)
                        j = n if end_q == -1 else end_q + 1
                        continue
                    if cleaned[j] == "(" and cleaned[j-1] != "\\": depth += 1
                    elif cleaned[j] == ")" and cleaned[j-1] != "\\": depth -= 1
                    j += 1
                subcmd = cleaned[i+2:j-1]
                extracted_substs.append(subcmd)
                cur.append(cleaned[i:j])
                i = j
                continue
            if c == "`":
                j = cleaned.find("`", i + 1)
                if j != -1:
                    subcmd = cleaned[i+1:j]
                    extracted_substs.append(subcmd)
                    cur.append(cleaned[i:j+1])
                    i = j + 1
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


SHELL_C = re.compile(r"^(\S*/)?(bash|sh|zsh|fish)(\s+-\w+)*\s+-l?c\s+(?P<script>.+)$", re.S)


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


CODE_EXT = re.compile(
    r"\.(rs|go|ts|tsx|js|jsx|mjs|cjs|py|pyi|c|cc|cpp|cxx|h|hh|hpp|hxx|swift|m|mm|kt|java|cs|rb|php|scala)$"
)
NOT_CODE_EXT = re.compile(r"\.(md|txt|log|jsonl?|ya?ml|toml|lock|csv|html?|xml|ini|cfg|conf|env|sql|sh)$")
IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
DEFINITION = re.compile(
    r"^(pub(\(crate\))?\s+)?(async\s+)?(export\s+)?"
    r"(fn|func|def|class|struct|enum|trait|interface|type|impl|protocol|extension)\s+(\([^)]*\)\s*)?"
    r"(?P<name>" + IDENT + r")"
)
GREP_OPTS_WITH_ARG = {"-e", "-f", "-A", "-B", "-C", "-m", "-g", "-t", "-T", "--glob", "--type",
                      "--type-not", "--regexp", "--file", "--max-count", "--context",
                      "--after-context", "--before-context", "--include", "--exclude"}

RESOURCE_EXTENSIONS = {
    # Data & config formats
    "json", "jsonl", "yaml", "yml", "toml", "xml", "csv", "tsv", "sql", "graphql", "gql", "proto",
    # Text, documentation & logs
    "md", "markdown", "txt", "rst", "adoc", "pdf", "log", "diff", "patch",
    # Config & lockfiles
    "lock", "ini", "cfg", "conf", "env", "plist", "properties", "map", "snap",
    # Web & styles
    "html", "htm", "css", "scss", "sass", "less", "svg",
    # Images & media
    "png", "jpg", "jpeg", "gif", "ico", "webp", "bmp", "tiff",
    # Archives & binaries
    "zip", "tar", "gz", "tgz", "bz2", "xz", "wasm", "bin", "dat", "data", "out",
    # Shell & scripts
    "sh", "bash", "zsh", "fish", "bat", "ps1",
    # Source code files
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyi", "c", "cc", "cpp", "cxx",
    "h", "hh", "hpp", "hxx", "swift", "kt", "kts", "java", "cs", "rb", "php", "scala", "zig",
    "lua", "r", "dart", "ex", "exs", "erl", "hs", "clj", "jl", "asm", "s",
}

SPECIAL_FILENAMES = {
    "makefile", "dockerfile", "containerfile", "license", "readme",
    "cargo.lock", "gemfile", "procfile", "vagrantfile",
}


def looks_like_symbol(name):
    """A code identifier rather than a word: snake_case, camelCase or PascalCase with an inner
    capital, or qualified (Type::method, pkg.Func). `error`, `TODO` and `Price` stay words.
    Filenames, extensions, and resource references (meta.json, Cargo.lock, README.md) stay words.
    Trailing-underscore prefixes (`test_`, `reserve_`) stay text patterns (#1119)."""
    if not re.fullmatch(IDENT + r"((::|\.)" + IDENT + r")*", name) or len(name) < 3:
        return False
    if name.endswith("_"):
        return False
    if "/" in name or "\\" in name:
        return False
    if name.lower() in SPECIAL_FILENAMES:
        return False
    if "::" in name:
        return True
    if "." in name:
        parts = name.split(".")
        ext = parts[-1].lower()
        if ext in RESOURCE_EXTENSIONS:
            return False
        if any(p.lower() in RESOURCE_EXTENSIONS for p in parts[1:]):
            return False
        return any("_" in p.strip("_") or bool(re.search(r"[a-z][A-Z]", p)) or p[:1].isupper() for p in parts[1:])
    if name.isupper():
        return False
    return "_" in name.strip("_") or bool(re.search(r"[a-z][A-Z]", name))


def symbol_in(pattern):
    """The symbol a grep pattern looks up, or None for a text search."""
    p = pattern.strip()
    for token in (r"\b", r"\<", r"\>", "^", "$", r"\s+", r"\s*"):
        p = p.replace(token, " " if token in (r"\s+", r"\s*") else "")
    p = re.sub(r"\s+", " ", p).strip()
    d = DEFINITION.match(p)
    if d and len(p) - d.end() <= 12:
        name = d.group("name")
        tail = p[d.end():].strip()
        if name.endswith("_") or len(name) < 3:
            return None
        if tail and not re.match(r"^(\\\(|\(|<|\{|:|$)", tail):
            return None
        return name
    q = re.sub(r"^(\\?\.|->)", "", p)
    q = re.sub(r"(\\\(|\()$", "", q)
    return q if looks_like_symbol(q) else None


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
            cur.append(cleaned[i:i+2])
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
            if cleaned[i:i+2] in ("&&", "||"):
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
            cur.append(pipeline[i:i+2])
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


def is_filename_or_fixture_pattern(pat, cwd, fixed_strings):
    """Whether `pat` names a filename, file extension, or existing workspace fixture (#765)."""
    p = pat.strip()
    if not p:
        return True
    if p.lower() in SPECIAL_FILENAMES:
        return True
    if ("/" in p or "\\" in p) and " " not in p and "|" not in p:
        if fixed_strings:
            return True
        base = os.path.basename(p)
        if "." in base and base.rsplit(".", 1)[-1].lower() in RESOURCE_EXTENSIONS:
            return True
        if cwd and (
            os.path.exists(os.path.join(cwd, p))
            or os.path.exists(os.path.join(cwd, "tests", "fixtures", p))
            or os.path.exists(os.path.join(cwd, "fixtures", p))
        ):
            return True
    if "." in p and " " not in p and "|" not in p and not p.startswith("."):
        ext = p.rsplit(".", 1)[-1].lower()
        if ext in RESOURCE_EXTENSIONS:
            return True
        if fixed_strings and re.fullmatch(r"[a-z0-9_-]{1,8}", ext):
            return True
    if fixed_strings and cwd and (
        os.path.exists(os.path.join(cwd, p))
        or os.path.exists(os.path.join(cwd, "tests", "fixtures", p))
        or os.path.exists(os.path.join(cwd, "fixtures", p))
    ):
        return True
    return False


def bre_to_ripgrep(pattern):
    """A basic regular expression (what grep and git grep read without -E) in ripgrep's syntax,
    which `prod-code grep` reads: `\\|`, `\\(`, `\\)`, `\\{`, `\\}`, `\\+` and `\\?` are operators there
    and lose their backslash, and the same characters unescaped are literals and gain one (#1581)."""
    out, i = [], 0
    while i < len(pattern):
        c = pattern[i]
        if c == "[":
            # A bracket expression is copied as it is, a leading `]` or `^]` included.
            end = i + 1
            if pattern[end:end + 1] == "^":
                end += 1
            if pattern[end:end + 1] == "]":
                end += 1
            end = pattern.find("]", end)
            if end < 0:
                out.append(pattern[i:])
                break
            out.append(pattern[i:end + 1])
            i = end + 1
            continue
        if c == "\\" and i + 1 < len(pattern):
            nxt = pattern[i + 1]
            out.append(nxt if nxt in "|(){}+?" else c + nxt)
            i += 2
            continue
        out.append("\\" + c if c in "|(){}+?" else c)
        i += 1
    return "".join(out)


def ripgrep_pattern(command, cwd, pat):
    """`pat`, from a grep in `command`, written the way `prod-code grep` reads it (#1581)."""
    for patterns, _paths, _recursive, fixed_strings, basic in _iter_grep_invocations(command, cwd):
        if pat in patterns:
            if fixed_strings:
                return re.sub(r"([\\.+*?()|\[\]{}^$])", r"\\\1", pat)
            return bre_to_ripgrep(pat) if basic else pat
    return pat


def _iter_grep_invocations(command, cwd):
    """Yields (patterns, paths, recursive, fixed_strings, basic) for grep/rg/git-grep over source
    files in `command`; `basic` when the patterns are basic regular expressions."""
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
        if tool in ("python", "python3", "node", "ruby", "perl", "sed", "awk", "tee", "cat", "patch"):
            preceded_by_script = True
            continue
        if tool in ("bash", "sh", "zsh", "fish"):
            # Codex sends its shell calls as `bash -lc '<script>'`.
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
                patterns.append(args[i + 1]); i += 2; continue
            if a in GREP_OPTS_WITH_ARG and i + 1 < len(args):
                i += 2; continue
            if a == "--":
                rest.extend(args[i + 1:])
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
                i += 1; continue
            rest.append(a); i += 1
        if not patterns and rest:
            patterns, rest = [rest[0]], rest[1:]
        if not patterns:
            continue
        paths = rest
        # A single-file non-recursive grep immediately following an inline script or heredoc
        # in the same compound command (e.g. `python3 - <<'EOF' ... EOF\ngrep -c sym file.go`)
        # is a post-edit verification check on the modified file, not codebase discovery (#1125).
        if preceded_by_script and not recursive and paths:
            if all(not os.path.isdir(os.path.join(cwd or ".", os.path.expanduser(x))) for x in paths):
                continue
        if paths:
            code = [x for x in paths if CODE_EXT.search(x) or (os.path.isdir(os.path.join(cwd or ".", os.path.expanduser(x))) and not re.search(r"(^|/)(\.claude|node_modules|target|\.build|logs?|tmp)(/|$)", x))]
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
    for patterns, _paths, _recursive, fixed_strings, _basic in _iter_grep_invocations(command, cwd):
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
    for patterns, paths, _recursive, fixed_strings, _basic in _iter_grep_invocations(command, cwd):
        target_path = None
        for p in paths:
            if p != "." and not p.startswith("*.") and not re.fullmatch(r"\.[a-z0-9]+", p, re.I):
                target_path = p
                break
        for pat in patterns:
            if is_filename_or_fixture_pattern(pat, cwd, fixed_strings):
                continue
            if symbol_in(pat) is not None:
                continue
            return pat, target_path
    return None


def _unwrap_shell_words(words):
    """Returns the command index after assignments and common shell wrappers.

    Also reports whether the caller explicitly requested the one-command source-read
    fallback, either in the hook environment or as a leading command assignment.
    """
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
                if option in (
                    "-i",
                    "--ignore-environment",
                    "-0",
                    "--null",
                    "-v",
                    "--debug",
                ):
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
                    "-u",
                    "--user",
                    "-g",
                    "--group",
                    "-h",
                    "--host",
                    "-p",
                    "--prompt",
                    "-C",
                    "--chdir",
                    "-r",
                    "--role",
                    "-t",
                    "--type",
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
                if words[i] in ("-o", "--output", "-f", "--format"):
                    i += 2
                else:
                    i += 1
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

        # Here-docs and here-strings provide stdin, not a source-file operand.
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

        # Ignore output redirections: they write a result but do not make sed read
        # the redirected file as source input.
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
            # GNU and BSD sed both accept an optional backup suffix attached to -i.
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
            # Handle combined short flags such as -ne and attached script forms
            # such as -ep. In-place mode is allowed regardless of its operands.
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

    # With no -e/-f option, sed's first positional operand is its program.
    files = positional if explicit_script else positional[1:]
    return files + redirected_input


def sed_source_target(command, cwd, depth=0):
    """Returns the source-code file sed reads, or None for stdin/resources/edits."""
    if depth >= 3:
        return None

    for part in split_shell_segments(command):
        try:
            words = shlex.split(part)
        except ValueError:
            continue
        if not words:
            continue

        index, _fallback = _unwrap_shell_words(words)
        if index >= len(words):
            continue

        remaining = shlex.join(words[index:])
        nested = SHELL_C.match(remaining)
        if nested:
            script = nested.group("script").strip()
            if len(script) >= 2 and script[0] == script[-1] and script[0] in "'\"":
                script = script[1:-1]
            target = sed_source_target(script, cwd, depth + 1)
            if target:
                return target
            continue

        if os.path.basename(words[index]).lower() not in ("sed", "gsed"):
            continue

        for path in _sed_source_targets(words[index + 1 :]):
            if not path or path == "-" or path in ("/dev/stdin", "/dev/fd/0"):
                continue
            expanded = os.path.expanduser(path)
            resolved = (
                expanded
                if os.path.isabs(expanded)
                else os.path.join(cwd or os.getcwd(), expanded)
            )
            if CODE_EXT.search(resolved.lower()) or CODE_EXT.search(
                os.path.realpath(resolved).lower()
            ):
                return path

    return None


def source_sed_fallback_requested(command, cwd=None):
    """An explicit one-command opt-in for source reads when prod-code is unavailable."""
    return _source_sed_fallback_requested(command, cwd or os.getcwd(), 0)


def _source_sed_fallback_requested(command, cwd, depth):
    if depth >= 3:
        return False
    for part in split_shell_segments(command):
        try:
            words = shlex.split(part)
        except ValueError:
            continue
        index, fallback = _unwrap_shell_words(words)
        if index >= len(words):
            continue
        remaining = shlex.join(words[index:])
        nested = SHELL_C.match(remaining)
        if nested:
            script = nested.group("script").strip()
            if len(script) >= 2 and script[0] == script[-1] and script[0] in "'\"":
                script = script[1:-1]
            if fallback and sed_source_target(script, cwd, depth + 1):
                return True
            if _source_sed_fallback_requested(script, cwd, depth + 1):
                return True
            continue
        if fallback and sed_source_target(remaining, cwd, depth + 1):
            return True
    return False


def asked_prod_code(transcript, symbol, agent=None):
    """Whether this session already asked prod-code about `symbol`: a line of its transcript
    that is a prod-code call (MCP code_* tool or the prod-code CLI) and names the symbol. Only
    the last 2 MB are read. Without a transcript (interactive human), returns True unless agent is specified."""
    if not transcript or not os.path.isfile(transcript):
        return agent is None
    bare = symbol.split("::")[-1].split(".")[-1]
    try:
        with open(transcript, "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 2_000_000))
            tail = f.read().decode("utf-8", "ignore")
    except OSError:
        return agent is None
    for line in tail.splitlines():
        if bare in line and re.search(r"prod-code__code_|prod_code__code_|\"code_(definition|references|symbols|search|grep|hover|callers|slice|outline)\"|prod-code\s+(def|refs|symbols|search|grep|hover|callers|slice|outline)\b", line):
            return True
    return False


def reported_prod_code_issue(transcript, symbol, agent=None):
    """Whether this session filed an issue about prod-code via code_report_issue or
    the prod-code report-issue CLI. Without a transcript (interactive human), returns True unless agent is specified."""
    if not transcript or not os.path.isfile(transcript):
        return agent is None
    try:
        with open(transcript, "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 2_000_000))
            tail = f.read().decode("utf-8", "ignore")
    except OSError:
        return agent is None
    for line in tail.splitlines():
        if re.search(r"prod-code__code_report_issue|prod_code__code_report_issue|\"code_report_issue\"|prod-code\s+report-issue\b", line):
            return True
    return False


def log(entry):
    try:
        with open(LOG, "a") as f:
            f.write(json.dumps(entry) + "\n")
    except OSError:
        pass


RENAME_STATE_DIR = os.environ.get("PROD_CODE_RENAME_STATE_DIR") or os.path.expanduser("~/.cache/prod-code-renames")
KEYWORDS = {
    "if", "else", "elif", "for", "while", "loop", "match", "case", "switch",
    "return", "break", "continue", "yield", "let", "mut", "var", "val", "const",
    "fn", "func", "function", "def", "pub", "private", "protected", "class",
    "struct", "enum", "trait", "interface", "type", "impl", "import", "from",
    "export", "package", "use", "mod", "crate", "self", "super", "this",
    "true", "false", "null", "nil", "none", "undefined", "async", "await",
}
IDENT_TOKEN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
HTML_INTRINSIC_TAGS = frozenset({
    "div", "details", "summary", "span", "section", "article", "aside", "header",
    "footer", "nav", "main", "p", "a", "ul", "ol", "li", "dl", "dt", "dd", "table",
    "thead", "tbody", "tfoot", "tr", "td", "th", "form", "fieldset", "legend",
    "label", "input", "button", "select", "option", "textarea", "dialog", "figure",
    "figcaption", "blockquote", "pre", "code", "h1", "h2", "h3", "h4", "h5", "h6",
})


def single_symbol_rename(old_str, new_str):
    """Returns (old_sym, new_sym) if both are single valid identifier tokens and differ,
    or if the edit is a contextual rename where exactly one identifier symbol changed."""
    old_s = (old_str or "").strip()
    new_s = (new_str or "").strip()
    if not old_s or not new_s or old_s == new_s:
        return None
    # 1. Bare naked identifier tokens
    if IDENT_TOKEN_RE.fullmatch(old_s) and IDENT_TOKEN_RE.fullmatch(new_s):
        if len(old_s) >= 2 and len(new_s) >= 2 and old_s.lower() not in KEYWORDS and new_s.lower() not in KEYWORDS:
            return old_s, new_s
        return None
    # 2. Contextual single-symbol replacement (e.g. `let h = fnv1a_64(b);` -> `let h = normalized_source_hash(b);`)
    old_tokens = set(re.findall(r"\b[A-Za-z_][A-Za-z0-9_]{1,}\b", old_s)) - KEYWORDS
    new_tokens = set(re.findall(r"\b[A-Za-z_][A-Za-z0-9_]{1,}\b", new_s)) - KEYWORDS
    removed = old_tokens - new_tokens
    added = new_tokens - old_tokens
    if len(removed) == 1 and len(added) == 1:
        old_sym = list(removed)[0]
        new_sym = list(added)[0]
        if old_sym.isdigit() or new_sym.isdigit() or len(old_sym) < 2 or len(new_sym) < 2:
            return None
        if (
            old_sym in HTML_INTRINSIC_TAGS
            and new_sym in HTML_INTRINSIC_TAGS
            and re.search(r"<\s*/?\s*" + re.escape(old_sym) + r"\b", old_s)
            and re.search(r"<\s*/?\s*" + re.escape(new_sym) + r"\b", new_s)
        ):
            return None
        simulated = re.sub(r"\b" + re.escape(old_sym) + r"\b", new_sym, old_s)
        if simulated.strip() == new_s.strip():
            return old_sym, new_sym
    return None


def check_multi_file_rename(conv_id, file_path, old_sym, new_sym, repo_root=None, now=None):
    """Returns (True, other_file) if (old_sym, new_sym) was already renamed in another file
    within the same repo within 30m, else (False, None). Serialized via interprocess file lock."""
    if now is None:
        now = int(time.time())
    try:
        os.makedirs(RENAME_STATE_DIR, mode=0o700, exist_ok=True)
        try:
            os.chmod(RENAME_STATE_DIR, 0o700)
        except OSError:
            pass
    except OSError:
        pass

    clean_id = re.sub(r"[^A-Za-z0-9_-]", "_", conv_id or "default")
    canon_repo = os.path.abspath(repo_root) if repo_root else ""
    if canon_repo:
        repo_slug = re.sub(r"[^A-Za-z0-9_-]", "_", os.path.basename(canon_repo))
        scope_key = f"{clean_id}_{repo_slug}"
    else:
        scope_key = clean_id

    lock_file = os.path.join(RENAME_STATE_DIR, f"rename_{scope_key}.lock")
    state_file = os.path.join(RENAME_STATE_DIR, f"rename_{scope_key}.json")

    # Expire stale session files older than 30m (only data files, never lock files)
    try:
        for fname in os.listdir(RENAME_STATE_DIR):
            if fname.startswith("rename_") and fname.endswith(".json"):
                fpath = os.path.join(RENAME_STATE_DIR, fname)
                try:
                    if now - int(os.path.getmtime(fpath)) > 1800:
                        os.remove(fpath)
                except OSError:
                    pass
    except OSError:
        pass

    lock_fd = None
    try:
        flags = os.O_RDWR | os.O_CREAT
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        lock_fd = os.open(lock_file, flags, 0o600)
        fcntl.flock(lock_fd, fcntl.LOCK_EX)
    except Exception:
        if lock_fd is not None:
            try:
                os.close(lock_fd)
            except OSError:
                pass
        # Fail safe rather than running the state update unsynchronized
        return False, None

    try:
        entries = []
        try:
            if os.path.exists(state_file):
                with open(state_file, "r") as f:
                    data = json.load(f)
                    entries = data.get("entries", [])
        except Exception:
            entries = []

        # Prune entries older than 30 minutes (1800s)
        entries = [e for e in entries if isinstance(e, dict) and now - e.get("ts", 0) <= 1800]

        abs_file = os.path.abspath(file_path)

        other_file = None
        for e in entries:
            if e.get("sym") == old_sym and e.get("new") == new_sym:
                stored_repo = e.get("repo", "")
                if canon_repo and stored_repo and canon_repo != stored_repo:
                    continue
                prev_file = e.get("file")
                if prev_file and os.path.abspath(prev_file) != abs_file:
                    other_file = prev_file
                    break

        if other_file:
            return True, other_file

        entries.append({"sym": old_sym, "new": new_sym, "file": abs_file, "repo": canon_repo, "ts": now})

        # Atomic replace via temporary file in the same directory
        temp_file = os.path.join(RENAME_STATE_DIR, f"tmp_{scope_key}_{os.getpid()}_{time.time_ns()}.json")
        try:
            wflags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
            if hasattr(os, "O_NOFOLLOW"):
                wflags |= os.O_NOFOLLOW
            fd = os.open(temp_file, wflags, 0o600)
            with os.fdopen(fd, "w") as f:
                json.dump({"entries": entries}, f)
            os.replace(temp_file, state_file)
        except Exception:
            if os.path.exists(temp_file):
                try:
                    os.remove(temp_file)
                except OSError:
                    pass

        return False, None
    finally:
        if lock_fd is not None:
            try:
                if hasattr(fcntl, "flock"):
                    fcntl.flock(lock_fd, fcntl.LOCK_UN)
                os.close(lock_fd)
            except OSError:
                pass


def parse_patch_edits(patch_text, env_override=False):
    """Parses Codex apply_patch and unified diff formats, returning
    [(file_path, old_content, new_content, is_override), ...]."""
    edits = []
    if not isinstance(patch_text, str) or not patch_text:
        return edits

    is_override = env_override or ("PROD_CODE_MANUAL_RENAME=1" in patch_text)
    current_file = None
    old_lines = []
    new_lines = []

    def flush_hunk():
        nonlocal current_file, old_lines, new_lines
        if current_file and (old_lines or new_lines):
            edits.append((current_file, "\n".join(old_lines), "\n".join(new_lines), is_override))
            old_lines = []
            new_lines = []

    for line in patch_text.splitlines():
        if line.startswith("*** Update File: "):
            flush_hunk()
            current_file = line[len("*** Update File: "):].strip()
        elif line.startswith("*** Add File: ") or line.startswith("*** Delete File: "):
            flush_hunk()
            current_file = None
        elif line.startswith("--- a/") or line.startswith("--- "):
            flush_hunk()
            parts = line.split(None, 1)
            p = parts[1] if len(parts) > 1 else ""
            if p.startswith("a/"):
                p = p[2:]
            current_file = p
        elif line.startswith("+++ b/") or line.startswith("+++ "):
            parts = line.split(None, 1)
            p = parts[1] if len(parts) > 1 else ""
            if p.startswith("b/"):
                p = p[2:]
            current_file = p
        elif current_file:
            if line.startswith("-") and not line.startswith("---"):
                old_lines.append(line[1:])
            elif line.startswith("+") and not line.startswith("+++"):
                new_lines.append(line[1:])

    flush_hunk()
    return edits


def file_edits_from_payload(payload):
    """Returns list of (file_path, old_content, new_content, is_override) for edit tool calls."""
    env_override = os.environ.get("PROD_CODE_MANUAL_RENAME") == "1"

    call = payload.get("toolCall")
    if isinstance(call, dict):
        name = call.get("name") or ""
        args = call.get("args") or {}
        if name in ("replace_file_content", "edit_file", "modify_file"):
            target_file = args.get("TargetFile") or args.get("target_file") or args.get("path") or args.get("file_path") or ""
            target_content = args.get("TargetContent") or args.get("target_content") or args.get("old_string") or args.get("old_str") or ""
            replacement = args.get("ReplacementContent") or args.get("replacement_content") or args.get("new_string") or args.get("new_str") or ""
            desc = f"{args.get('Description', '')} {args.get('Instruction', '')}"
            override = env_override or ("PROD_CODE_MANUAL_RENAME=1" in desc)
            return [(target_file, target_content, replacement, override)]
        return []

    tool_name = payload.get("tool_name") or payload.get("tool") or ""
    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return []

    if tool_name in ("Edit", "str_replace_editor", "edit_file", "replace_file_content"):
        target_file = tool_input.get("file_path") or tool_input.get("path") or tool_input.get("target_file") or ""
        target_content = tool_input.get("old_string") or tool_input.get("old_str") or tool_input.get("TargetContent") or ""
        replacement = tool_input.get("new_string") or tool_input.get("new_str") or tool_input.get("ReplacementContent") or ""
        override = env_override or any(
            isinstance(v, str) and "PROD_CODE_MANUAL_RENAME=1" in v
            for v in tool_input.values()
        )
        return [(target_file, target_content, replacement, override)]

    if tool_name in ("apply_patch", "patch"):
        patch_text = tool_input.get("cmd") or tool_input.get("patch") or tool_input.get("command") or ""
        return parse_patch_edits(patch_text, env_override)

    return []


def file_edit_details(payload):
    """Returns (file_path, old_content, new_content, is_override) for edit tool calls, or None."""
    edits = file_edits_from_payload(payload)
    return edits[0] if edits else None


def shell_command(payload):
    """The shell command a tool call runs, whichever agent sent it: Claude Code's Bash
    (`command`), Gemini CLI's run_shell_command (`command`), Codex's shell tools (`command` as a
    string or an argv list, or `cmd`). None for a tool that runs no command."""
    call = payload.get("toolCall")
    if isinstance(call, dict):
        # Antigravity: {"toolCall": {"name": "run_command", "args": {"CommandLine": "..."}}}
        if call.get("name") not in ("run_command", "bash", "exec", "sh"):
            return None
        command = (call.get("args") or {}).get("CommandLine")
        return command if isinstance(command, str) and command.strip() else None

    tool_name = payload.get("tool_name") or payload.get("tool") or ""
    SHELL_TOOLS = {"bash", "sh", "zsh", "fish", "exec", "shell", "run_shell_command", "run_command"}
    if tool_name and tool_name.lower() not in SHELL_TOOLS:
        return None

    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return None
    command = tool_input.get("command", tool_input.get("cmd"))
    if isinstance(command, list):
        command = " ".join(shlex.quote(str(c)) for c in command)
    return command if isinstance(command, str) and command.strip() else None


def main():
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw)
    except Exception:
        return 0
    agy = isinstance(payload.get("toolCall"), dict)

    # 1. Inspect file edit tools for multi-file symbol renames
    edits = file_edits_from_payload(payload)
    if edits:
        cwd = os.getcwd()
        if agy:
            paths = payload.get("workspacePaths") or []
            cwd = paths[0] if paths else os.getcwd()
            agent = "antigravity"
            conv_id = payload.get("conversationId") or "default"
        else:
            cwd = payload.get("cwd") or os.getcwd()
            agent = "gemini" if payload.get("hook_event_name") == "BeforeTool" else payload.get("tool_name", "")
            conv_id = payload.get("conversation_id") or payload.get("session_id") or "default"

        for target_file, old_content, replacement, override in edits:
            resolved_file = target_file if os.path.isabs(target_file) else os.path.join(cwd, target_file)
            repo_root = git_repo_root(resolved_file) or git_repo_root(cwd)
            if resolved_file and CODE_EXT.search(resolved_file) and repo_root is not None:
                sym_pair = single_symbol_rename(old_content, replacement)
                if sym_pair:
                    old_sym, new_sym = sym_pair
                    if override:
                        log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "override", "cwd": cwd, "symbol": old_sym, "file": resolved_file})
                    else:
                        is_multi, prev_file = check_multi_file_rename(conv_id, resolved_file, old_sym, new_sym, repo_root=repo_root)
                        if is_multi:
                            log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "deny", "cwd": cwd, "symbol": old_sym, "file": resolved_file, "prev_file": prev_file})
                            prev_name = os.path.basename(prev_file)
                            cur_name = os.path.basename(resolved_file)
                            reason = (
                                f"Multi-file symbol rename detected: replacing `{old_sym}` with `{new_sym}` across files "
                                f"(`{prev_name}` and `{cur_name}`). Do not edit files manually across the workspace. "
                                f"Use the AST refactoring tool instead: "
                                f"code_rename {{path: \"{cur_name}\", line: ..., character: ..., new_name: \"{new_sym}\"}} "
                                f"(CLI: prod-code rename <file> <line> <col> {new_sym}). "
                                f"It updates all declarations, references, and imports across the entire workspace atomically and safely. "
                                f"If this is not an automated symbol rename, include PROD_CODE_MANUAL_RENAME=1 in your edit description/explanation "
                                f"or set PROD_CODE_MANUAL_RENAME=1 in the environment."
                            )
                            if agent in ("gemini", "antigravity"):
                                print(json.dumps({"decision": "deny", "reason": reason}))
                            else:
                                print(json.dumps({
                                    "hookSpecificOutput": {
                                        "hookEventName": "PreToolUse",
                                        "permissionDecision": "deny",
                                        "permissionDecisionReason": reason,
                                    }
                                }))
                            return 0
                        else:
                            log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "first-file-tracked", "cwd": cwd, "symbol": old_sym, "file": resolved_file})
        return 0

    command = shell_command(payload)
    if command is None:
        if agy:
            pass
        return 0
    if agy:
        args = payload["toolCall"].get("args") or {}
        paths = payload.get("workspacePaths") or []
        cwd = args.get("Cwd") or (paths[0] if paths else os.getcwd())
        agent = "antigravity"
    else:
        cwd = payload.get("cwd") or os.getcwd()
        agent = "gemini" if payload.get("hook_event_name") == "BeforeTool" else payload.get("tool_name", "")
    reason = verdict(command)
    if reason:
        log({"ts": int(time.time()), "agent": agent, "rule": "local-build", "decision": "deny", "cwd": cwd, "cmd": command[:300]})
    transcript = payload.get("transcript_path") or payload.get("transcriptPath")
    conv_id = payload.get("conversationId") or payload.get("conversation_id")
    if not transcript and conv_id:
        cand = os.path.expanduser(f"~/.gemini/antigravity-cli/brain/{conv_id}/.system_generated/logs/transcript.jsonl")
        if os.path.isfile(cand):
            transcript = cand
    sess_id = payload.get("session_id") or payload.get("sessionId")
    if not transcript and sess_id:
        cwd_str = cwd or os.getcwd()
        repo = git_repo_root(cwd_str)
        candidates = []
        for path_val in (cwd_str, repo):
            if path_val:
                slug = path_val.replace("/", "-")
                candidates.append(os.path.expanduser(f"~/.claude/projects/{slug}/{sess_id}.jsonl"))
                candidates.append(os.path.expanduser(f"~/.claude/projects/-{slug.lstrip('-')}/{sess_id}.jsonl"))
        for cand in candidates:
            if os.path.isfile(cand):
                transcript = cand
                break

    if not reason:
        found = symbol_grep(command, cwd)
        if found is None and "PROD_CODE_GREP=1" in command:
            again = symbol_grep(command.replace("PROD_CODE_GREP=1", ""), cwd)
            if again:
                sym = again[0]
                if not asked_prod_code(transcript, sym, agent=agent):
                    log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override-refused", "cwd": cwd, "symbol": sym, "cmd": command[:300]})
                    reason = (
                        f"Grep for symbol `{sym}` is blocked. Ask prod-code first: "
                        f"code_definition {{symbol: \"{sym}\"}}, code_symbols {{query: \"{sym}\"}}, "
                        f"or code_grep {{pattern: \"{sym}\"}} (`prod-code grep \"{sym}\"`)."
                    )
                elif not reported_prod_code_issue(transcript, sym, agent=agent):
                    log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override-refused", "cwd": cwd, "symbol": sym, "cmd": command[:300]})
                    reason = (
                        f"Grep for symbol `{sym}` is blocked. If prod-code failed to resolve `{sym}`, "
                        f"you must report the bug first: code_report_issue {{title: \"Symbol not found: {sym}\", body: \"...\"}}."
                    )
                else:
                    log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override", "cwd": cwd, "symbol": sym, "cmd": command[:300]})
        if found:
            sym, pat = found
            log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "deny", "cwd": cwd, "symbol": sym, "pattern": pat, "cmd": command[:300]})
            rg = ripgrep_pattern(command, cwd, pat)
            reason = (
                f"Grep for code symbol `{sym}` is blocked (`{pat}`). Use prod-code AST tools: "
                f"code_definition {{symbol: \"{sym}\"}}, code_references {{symbol: \"{sym}\"}}, or code_symbols {{query: \"{sym}\"}} "
                f"(or for literal/regex text search, use code_grep {{pattern: \"{rg}\"}} / `prod-code grep \"{rg}\"`). "
                f"If prod-code returns empty or errors, file a bug: code_report_issue {{title: \"Symbol not found: {sym}\", body: \"...\"}}."
            )
        else:
            found_text = text_grep(command, cwd)
            if found_text is not None:
                pat, target_path = found_text
                log({"ts": int(time.time()), "agent": agent, "rule": "text-grep", "decision": "deny", "cwd": cwd, "pattern": pat, "cmd": command[:300]})
                # `prod-code grep` reads ripgrep's syntax, not grep's basic one (#1581).
                rg = ripgrep_pattern(command, cwd, pat)
                if target_path:
                    mcp_call = f'code_grep {{pattern: "{rg}", path: "{target_path}"}}'
                    cli_call = f'prod-code grep "{rg}" {target_path}'
                else:
                    mcp_call = f'code_grep {{pattern: "{rg}"}}'
                    cli_call = f'prod-code grep "{rg}"'
                reason = (
                    f"Grep for text pattern `{pat}` over source files is blocked. "
                    f"Use prod-code literal/regex text search instead: {mcp_call} (CLI: `{cli_call}`). "
                    f"It runs ripgrep on the build node and returns file:line:col with the enclosing AST symbol (`[in Type::method]`). "
                    f"For AST symbol lookups use code_symbols / code_definition / code_references; for concept search use code_search."
                )
    if not reason:
        source_target = sed_source_target(command, cwd)
        if source_target:
            if source_sed_fallback_requested(command, cwd):
                log(
                    {
                        "ts": int(time.time()),
                        "agent": agent,
                        "rule": "source-sed",
                        "decision": "override",
                        "cwd": cwd,
                        "file": source_target,
                        "cmd": command[:300],
                    }
                )
            else:
                log(
                    {
                        "ts": int(time.time()),
                        "agent": agent,
                        "rule": "source-sed",
                        "decision": "deny",
                        "cwd": cwd,
                        "file": source_target,
                        "cmd": command[:300],
                    }
                )
                reason = (
                    f"Direct `sed` reads of source-code files are blocked (`{source_target}`). "
                    f"Use code_outline {{path: {json.dumps(source_target)}}} to inspect file structure, "
                    f"code_definition for a named declaration, or code_grep for literal text. "
                    f"If prod-code is unavailable, prefix this single read with "
                    f"`PROD_CODE_SOURCE_FALLBACK=1`; fallback use is logged. Report a persistent prod-code outage."
                )
    if not reason and agy:
        # No decision at all: "allow" would skip Antigravity's own permission prompt.
        pass
    if reason:
        if agent in ("gemini", "antigravity"):
            print(json.dumps({"decision": "deny", "reason": reason}))
        else:
            print(
                json.dumps(
                    {
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": reason,
                        }
                    }
                )
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
