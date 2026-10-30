# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Multi-file symbol rename classification and locked interprocess state tracking."""

import fcntl
import json
import os
import re
import time

from .payload import (
    SHELL_TOOLS,
    file_edit_details,
    file_edits_from_payload,
    parse_patch_edits,
    shell_command,
)

__all__ = [
    "DEFAULT_RENAME_STATE_DIR",
    "HTML_INTRINSIC_TAGS",
    "IDENT_TOKEN_RE",
    "KEYWORDS",
    "SHELL_TOOLS",
    "check_multi_file_rename",
    "file_edit_details",
    "file_edits_from_payload",
    "parse_patch_edits",
    "shell_command",
    "single_symbol_rename",
]

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
