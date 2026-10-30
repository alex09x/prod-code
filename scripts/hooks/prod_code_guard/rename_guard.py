# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Multi-file symbol rename tracking and agent PreToolUse payload extraction."""

import fcntl
import json
import os
import re
import shlex
import time

DEFAULT_RENAME_STATE_DIR = os.environ.get(
    "PROD_CODE_RENAME_STATE_DIR"
) or os.path.expanduser("~/.cache/prod-code-renames")

KEYWORDS = {
    "if",
    "else",
    "elif",
    "for",
    "while",
    "loop",
    "match",
    "case",
    "switch",
    "return",
    "break",
    "continue",
    "yield",
    "let",
    "mut",
    "var",
    "val",
    "const",
    "fn",
    "func",
    "function",
    "def",
    "pub",
    "private",
    "protected",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "type",
    "impl",
    "import",
    "from",
    "export",
    "package",
    "use",
    "mod",
    "crate",
    "self",
    "super",
    "this",
    "true",
    "false",
    "null",
    "nil",
    "none",
    "undefined",
    "async",
    "await",
}
IDENT_TOKEN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
HTML_INTRINSIC_TAGS = frozenset(
    {
        "div",
        "details",
        "summary",
        "span",
        "section",
        "article",
        "aside",
        "header",
        "footer",
        "nav",
        "main",
        "p",
        "a",
        "ul",
        "ol",
        "li",
        "dl",
        "dt",
        "dd",
        "table",
        "thead",
        "tbody",
        "tfoot",
        "tr",
        "td",
        "th",
        "form",
        "fieldset",
        "legend",
        "label",
        "input",
        "button",
        "select",
        "option",
        "textarea",
        "dialog",
        "figure",
        "figcaption",
        "blockquote",
        "pre",
        "code",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
    }
)
SHELL_TOOLS = {
    "bash",
    "sh",
    "zsh",
    "fish",
    "exec",
    "exec_command",
    "shell",
    "run_shell_command",
    "run_command",
}


def single_symbol_rename(old_str, new_str):
    """Returns (old_sym, new_sym) if both are single valid identifier tokens and differ,
    or if the edit is a contextual rename where exactly one identifier symbol changed."""
    old_s = (old_str or "").strip()
    new_s = (new_str or "").strip()
    if not old_s or not new_s or old_s == new_s:
        return None
    if IDENT_TOKEN_RE.fullmatch(old_s) and IDENT_TOKEN_RE.fullmatch(new_s):
        if (
            len(old_s) >= 2
            and len(new_s) >= 2
            and old_s.lower() not in KEYWORDS
            and new_s.lower() not in KEYWORDS
        ):
            return old_s, new_s
        return None
    old_tokens = set(re.findall(r"\b[A-Za-z_][A-Za-z0-9_]{1,}\b", old_s)) - KEYWORDS
    new_tokens = set(re.findall(r"\b[A-Za-z_][A-Za-z0-9_]{1,}\b", new_s)) - KEYWORDS
    removed = old_tokens - new_tokens
    added = new_tokens - old_tokens
    if len(removed) == 1 and len(added) == 1:
        old_sym = next(iter(removed))
        new_sym = next(iter(added))
        if (
            old_sym.isdigit()
            or new_sym.isdigit()
            or len(old_sym) < 2
            or len(new_sym) < 2
        ):
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


def check_multi_file_rename(
    conv_id, file_path, old_sym, new_sym, repo_root=None, now=None, state_dir=None
):
    """Returns (True, other_file) if (old_sym, new_sym) was already renamed in another file
    within the same repo within 30m, else (False, None). Serialized via interprocess file lock."""
    active_dir = state_dir or DEFAULT_RENAME_STATE_DIR
    if now is None:
        now = int(time.time())
    try:
        os.makedirs(active_dir, mode=0o700, exist_ok=True)
        try:
            os.chmod(active_dir, 0o700)
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

    lock_file = os.path.join(active_dir, f"rename_{scope_key}.lock")
    state_file = os.path.join(active_dir, f"rename_{scope_key}.json")

    try:
        for fname in os.listdir(active_dir):
            if fname.startswith("rename_") and fname.endswith(".json"):
                fpath = os.path.join(active_dir, fname)
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
    except OSError:
        if lock_fd is not None:
            try:
                os.close(lock_fd)
            except OSError:
                pass
        return False, None

    try:
        entries = []
        try:
            if os.path.exists(state_file):
                with open(state_file, "r") as f:
                    entries = json.load(f).get("entries", [])
        except (OSError, ValueError, AttributeError):
            entries = []

        entries = [
            e for e in entries if isinstance(e, dict) and now - e.get("ts", 0) <= 1800
        ]
        abs_file = os.path.abspath(file_path)

        for e in entries:
            if e.get("sym") == old_sym and e.get("new") == new_sym:
                stored_repo = e.get("repo", "")
                if canon_repo and stored_repo and canon_repo != stored_repo:
                    continue
                prev_file = e.get("file")
                if prev_file and os.path.abspath(prev_file) != abs_file:
                    return True, prev_file

        entries.append(
            {
                "sym": old_sym,
                "new": new_sym,
                "file": abs_file,
                "repo": canon_repo,
                "ts": now,
            }
        )

        temp_file = os.path.join(
            active_dir, f"tmp_{scope_key}_{os.getpid()}_{time.time_ns()}.json"
        )
        try:
            wflags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
            if hasattr(os, "O_NOFOLLOW"):
                wflags |= os.O_NOFOLLOW
            fd = os.open(temp_file, wflags, 0o600)
            with os.fdopen(fd, "w") as f:
                json.dump({"entries": entries}, f)
            os.replace(temp_file, state_file)
        except (OSError, TypeError, ValueError):
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
    """Parses Codex apply_patch and unified diff formats into
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
            edits.append(
                (current_file, "\n".join(old_lines), "\n".join(new_lines), is_override)
            )
            old_lines = []
            new_lines = []

    for line in patch_text.splitlines():
        if line.startswith("*** Update File: "):
            flush_hunk()
            current_file = line[len("*** Update File: ") :].strip()
        elif line.startswith(("*** Add File: ", "*** Delete File: ")):
            flush_hunk()
            current_file = None
        elif line.startswith(("--- a/", "--- ")):
            flush_hunk()
            parts = line.split(None, 1)
            p = parts[1] if len(parts) > 1 else ""
            current_file = p.removeprefix("a/")
        elif line.startswith(("+++ b/", "+++ ")):
            parts = line.split(None, 1)
            p = parts[1] if len(parts) > 1 else ""
            current_file = p.removeprefix("b/")
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
            target_file = (
                args.get("TargetFile")
                or args.get("target_file")
                or args.get("path")
                or args.get("file_path")
                or ""
            )
            target_content = (
                args.get("TargetContent")
                or args.get("target_content")
                or args.get("old_string")
                or args.get("old_str")
                or ""
            )
            replacement = (
                args.get("ReplacementContent")
                or args.get("replacement_content")
                or args.get("new_string")
                or args.get("new_str")
                or ""
            )
            desc = f"{args.get('Description', '')} {args.get('Instruction', '')}"
            override = env_override or ("PROD_CODE_MANUAL_RENAME=1" in desc)
            return [(target_file, target_content, replacement, override)]
        return []

    tool_name = payload.get("tool_name") or payload.get("tool") or ""
    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return []

    if tool_name in ("Edit", "str_replace_editor", "edit_file", "replace_file_content"):
        target_file = (
            tool_input.get("file_path")
            or tool_input.get("path")
            or tool_input.get("target_file")
            or ""
        )
        target_content = (
            tool_input.get("old_string")
            or tool_input.get("old_str")
            or tool_input.get("TargetContent")
            or ""
        )
        replacement = (
            tool_input.get("new_string")
            or tool_input.get("new_str")
            or tool_input.get("ReplacementContent")
            or ""
        )
        override = env_override or any(
            isinstance(v, str) and "PROD_CODE_MANUAL_RENAME=1" in v
            for v in tool_input.values()
        )
        return [(target_file, target_content, replacement, override)]

    if tool_name in ("apply_patch", "patch"):
        patch_text = (
            tool_input.get("cmd")
            or tool_input.get("patch")
            or tool_input.get("command")
            or ""
        )
        return parse_patch_edits(patch_text, env_override)

    return []


def file_edit_details(payload):
    """Returns (file_path, old_content, new_content, is_override) for edit tool calls, or None."""
    edits = file_edits_from_payload(payload)
    return edits[0] if edits else None


def shell_command(payload):
    """The shell command a tool call runs across Claude Code, Codex, Gemini CLI, and Antigravity."""
    call = payload.get("toolCall")
    if isinstance(call, dict):
        if call.get("name") not in ("run_command", "bash", "exec", "sh"):
            return None
        command = (call.get("args") or {}).get("CommandLine")
        return command if isinstance(command, str) and command.strip() else None

    tool_name = payload.get("tool_name") or payload.get("tool") or ""
    if tool_name and tool_name.lower() not in SHELL_TOOLS:
        return None

    tool_input = payload.get("tool_input") or {}
    if not isinstance(tool_input, dict):
        return None
    command = tool_input.get("command", tool_input.get("cmd"))
    if isinstance(command, list):
        command = " ".join(shlex.quote(str(c)) for c in command)
    return command if isinstance(command, str) and command.strip() else None
