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


HEREDOC_QUOTED = re.compile(r"<<-?\s*(['\"])(?P<delim>\w+)\1[^\n]*\n.*?\n\s*(?P=delim)\s*(?=\n|$)", re.S)
HEREDOC_UNQUOTED = re.compile(r"<<-?\s*(?P<delim>\w+)[^\n]*\n(?P<body>.*?)\n\s*(?P=delim)\s*(?=\n|$)", re.S)


def without_heredocs(command):
    """The command with every here-document body replaced with `<<heredoc`.
    For unquoted here-documents, command substitutions `$(...)` and backticks
    are preserved so nested executable commands can still be analyzed."""
    cleaned = HEREDOC_QUOTED.sub("<<heredoc", command)
    substs = []

    def handle_unquoted(m):
        body = m.group("body")
        if body:
            for sub in re.findall(r"\$\((.*?)\)|`([^`]*)`", body, re.S):
                substs.append(sub[0] or sub[1])
        return "<<heredoc"

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
    Filenames, extensions, and resource references (meta.json, Cargo.lock, README.md) stay words."""
    if not re.fullmatch(IDENT + r"((::|\.)" + IDENT + r")*", name) or len(name) < 3:
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
        return d.group("name")
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
    cleaned = without_heredocs(command)
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


def symbol_grep(command, cwd):
    """(symbol, pattern) when the command looks a symbol up over source files with grep/rg."""
    if "PROD_CODE_GREP=1" in command:
        return None
    for pipeline in split_pipelines(command):
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
        if tool in ("bash", "sh", "zsh", "fish"):
            # Codex sends its shell calls as `bash -lc '<script>'`.
            for j, a in enumerate(args[:-1]):
                if re.fullmatch(r"-[a-z]*c", a):
                    inner = symbol_grep(args[j + 1], cwd)
                    if inner:
                        return inner
            continue
        if tool == "git" and args[:1] == ["grep"]:
            tool, args = "git-grep", args[1:]
        if tool not in ("grep", "egrep", "fgrep", "rg", "ag", "git-grep"):
            continue
        recursive = tool != "grep" and tool != "egrep"
        fixed_strings = tool == "fgrep"
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
                i += 1; continue
            rest.append(a); i += 1
        if not patterns and rest:
            patterns, rest = [rest[0]], rest[1:]
        if not patterns:
            continue
        paths = rest
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
        for pat in patterns:
            if fixed_strings:
                if "/" in pat or "\\" in pat or pat.lower() in SPECIAL_FILENAMES:
                    continue
                if "." in pat:
                    ext = pat.rsplit(".", 1)[-1].lower()
                    if ext in RESOURCE_EXTENSIONS or re.fullmatch(r"[a-z0-9_-]{1,8}", ext):
                        continue
                if cwd and (os.path.exists(os.path.join(cwd, pat)) or os.path.exists(os.path.join(cwd, "tests", "fixtures", pat)) or os.path.exists(os.path.join(cwd, "fixtures", pat))):
                    continue
            sym = symbol_in(pat)
            if sym:
                return sym, pat
    return None


def asked_prod_code(transcript, symbol):
    """Whether this session already asked prod-code about `symbol`: a line of its transcript
    that is a prod-code call (MCP code_* tool or the prod-code CLI) and names the symbol. Only
    the last 2 MB are read. Without a transcript nothing can be shown, so the answer is yes."""
    if not transcript or not os.path.isfile(transcript):
        return True
    bare = symbol.split("::")[-1].split(".")[-1]
    try:
        with open(transcript, "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 2_000_000))
            tail = f.read().decode("utf-8", "ignore")
    except OSError:
        return True
    for line in tail.splitlines():
        if bare in line and re.search(r"prod-code__code_|prod_code__code_|\"code_(definition|references|symbols|search|hover|callers|slice|outline)\"|prod-code\s+(def|refs|symbols|search|hover|callers|slice|outline)\b", line):
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

    # Expire stale session files older than 30m
    try:
        for fname in os.listdir(RENAME_STATE_DIR):
            if fname.startswith("rename_") and (fname.endswith(".json") or fname.endswith(".lock")):
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
        if hasattr(fcntl, "flock"):
            fcntl.flock(lock_fd, fcntl.LOCK_EX)
    except Exception:
        lock_fd = None

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


def file_edit_details(payload):
    """Returns (file_path, old_content, new_content, is_override) for edit tool calls, or None."""
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
            return target_file, target_content, replacement, override
        return None

    tool_name = payload.get("tool_name") or payload.get("tool") or ""
    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return None
    if tool_name in ("Edit", "str_replace_editor", "edit_file", "replace_file_content"):
        target_file = tool_input.get("file_path") or tool_input.get("path") or tool_input.get("target_file") or ""
        target_content = tool_input.get("old_string") or tool_input.get("old_str") or tool_input.get("TargetContent") or ""
        replacement = tool_input.get("new_string") or tool_input.get("new_str") or tool_input.get("ReplacementContent") or ""
        override = env_override or any(
            isinstance(v, str) and "PROD_CODE_MANUAL_RENAME=1" in v
            for v in tool_input.values()
        )
        return target_file, target_content, replacement, override

    return None


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
    edit = file_edit_details(payload)
    if edit:
        target_file, old_content, replacement, override = edit
        cwd = os.getcwd()
        if agy:
            args = payload["toolCall"].get("args") or {}
            paths = payload.get("workspacePaths") or []
            cwd = paths[0] if paths else os.getcwd()
            agent = "antigravity"
            conv_id = payload.get("conversationId") or "default"
        else:
            cwd = payload.get("cwd") or os.getcwd()
            agent = "gemini" if payload.get("hook_event_name") == "BeforeTool" else payload.get("tool_name", "")
            conv_id = payload.get("conversation_id") or payload.get("session_id") or "default"

        repo_root = git_repo_root(target_file if target_file else cwd) or git_repo_root(cwd)
        if target_file and CODE_EXT.search(target_file) and repo_root is not None:
            sym_pair = single_symbol_rename(old_content, replacement)
            if sym_pair:
                old_sym, new_sym = sym_pair
                if override:
                    log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "override", "cwd": cwd, "symbol": old_sym, "file": target_file})
                else:
                    is_multi, prev_file = check_multi_file_rename(conv_id, target_file, old_sym, new_sym, repo_root=repo_root)
                    if is_multi:
                        log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "deny", "cwd": cwd, "symbol": old_sym, "file": target_file, "prev_file": prev_file})
                        prev_name = os.path.basename(prev_file)
                        cur_name = os.path.basename(target_file)
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
                        log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "first-file-tracked", "cwd": cwd, "symbol": old_sym, "file": target_file})
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
    if not reason:
        found = symbol_grep(command, cwd)
        if found is None and "PROD_CODE_GREP=1" in command:
            again = symbol_grep(command.replace("PROD_CODE_GREP=1", ""), cwd)
            if again and not asked_prod_code(transcript, again[0]):
                # An override is for a place where prod-code failed: it has to have been asked.
                log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override-refused", "cwd": cwd, "symbol": again[0], "cmd": command[:300]})
                sym = again[0]
                reason = (
                    f"PROD_CODE_GREP=1 is accepted only after prod-code was asked about `{sym}` in this session, and "
                    f"it was not. Ask it first: code_definition {{symbol: \"{sym}\"}}, code_references {{symbol: \"{sym}\"}} "
                    f"or code_symbols {{query: \"{sym}\"}} (CLI: prod-code def|refs --symbol {sym}). To read the whole "
                    f"definition, use code_definition and then read the file at the position it gives. If prod-code is "
                    f"wrong or empty, report it with code_report_issue; then the override works."
                )
            elif again:
                log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override", "cwd": cwd, "symbol": again[0], "cmd": command[:300]})
        if found:
            sym, pat = found
            log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "deny", "cwd": cwd, "symbol": sym, "pattern": pat, "cmd": command[:300]})
            reason = (
                f"`{pat}` looks up the symbol `{sym}`. Use prod-code, which resolves the symbol instead of matching text: "
                f"code_definition {{symbol: \"{sym}\"}}, code_references {{symbol: \"{sym}\"}}, or code_symbols {{query: \"{sym}\"}} "
                f"(CLI: prod-code def|refs --symbol {sym}, prod-code symbols {sym}). "
                "If prod-code answers wrong, empty or not at all, report it with code_report_issue; after prod-code "
                "has been asked about this symbol, PROD_CODE_GREP=1 in front of the grep lets it through."
            )
    if not reason and agy:
        # No decision at all: "allow" would skip Antigravity's own permission prompt.
        pass
    if reason:
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


if __name__ == "__main__":
    sys.exit(main())
