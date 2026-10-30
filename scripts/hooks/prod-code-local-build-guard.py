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
"""PreToolUse hook for Bash and Edit tools across Claude Code, Codex, Gemini CLI, and Antigravity.

Enforces remote execution for heavy builds/tests, routes source searches and reads to semantic
prod-code tools, blocks multi-file manual symbol renames, and redirects identifiable inline
Python source-file rewrites to contextual edit or refactoring tools.
"""

import json
import os
import sys
from pathlib import Path

_HOOK_DIR = str(Path(__file__).resolve().parent)
sys.path[:] = [
    _HOOK_DIR,
    *[p for p in sys.path if p not in ("", _HOOK_DIR, os.getcwd())],
]

from prod_code_guard.dispatch import (
    evaluate_edit_renames,
    evaluate_shell_rules,
    extract_shell_context,
)
from prod_code_guard.grep_guard import (
    asked_prod_code,
    bre_to_ripgrep,
    is_filename_or_fixture_pattern,
    looks_like_symbol,
    reported_prod_code_issue,
    ripgrep_pattern,
    symbol_grep,
    symbol_in,
    text_grep,
)
from prod_code_guard.python_write_guard import scripted_source_write_target
from prod_code_guard.rename_guard import (
    DEFAULT_RENAME_STATE_DIR,
    HTML_INTRINSIC_TAGS,
    IDENT_TOKEN_RE,
    KEYWORDS,
    SHELL_TOOLS,
    file_edit_details,
    file_edits_from_payload,
    parse_patch_edits,
    shell_command,
    single_symbol_rename,
)
from prod_code_guard.rename_guard import (
    check_multi_file_rename as _check_multi_file_rename,
)
from prod_code_guard.sed_guard import (
    sed_source_target,
    source_sed_fallback_requested,
)
from prod_code_guard.shell import (
    ALLOWED_ANYWHERE,
    CODE_EXT,
    HEAVY,
    NOT_CODE_EXT,
    REASON,
    SHELL_C,
    first_command_in_pipeline,
    git_repo_root,
    in_git_repo,
    segments,
    split_pipelines,
    split_shell_segments,
    verdict,
    without_heredocs,
)

__all__ = [
    "ALLOWED_ANYWHERE",
    "CODE_EXT",
    "HEAVY",
    "HTML_INTRINSIC_TAGS",
    "IDENT_TOKEN_RE",
    "KEYWORDS",
    "LOG",
    "NOT_CODE_EXT",
    "REASON",
    "RENAME_STATE_DIR",
    "SHELL_C",
    "SHELL_TOOLS",
    "asked_prod_code",
    "bre_to_ripgrep",
    "check_multi_file_rename",
    "file_edit_details",
    "file_edits_from_payload",
    "first_command_in_pipeline",
    "git_repo_root",
    "in_git_repo",
    "is_filename_or_fixture_pattern",
    "log",
    "looks_like_symbol",
    "main",
    "parse_patch_edits",
    "reported_prod_code_issue",
    "ripgrep_pattern",
    "scripted_source_write_target",
    "sed_source_target",
    "segments",
    "shell_command",
    "single_symbol_rename",
    "source_sed_fallback_requested",
    "split_pipelines",
    "split_shell_segments",
    "symbol_grep",
    "symbol_in",
    "text_grep",
    "verdict",
    "without_heredocs",
]

LOG = os.environ.get("PROD_CODE_GUARD_LOG") or os.path.expanduser(
    "~/.claude/hooks/prod-code-guard.jsonl"
)
RENAME_STATE_DIR = DEFAULT_RENAME_STATE_DIR


def log(entry):
    try:
        with open(LOG, "a") as f:
            f.write(json.dumps(entry) + "\n")
    except OSError:
        pass


def check_multi_file_rename(
    conv_id, file_path, old_sym, new_sym, repo_root=None, now=None
):
    return _check_multi_file_rename(
        conv_id,
        file_path,
        old_sym,
        new_sym,
        repo_root=repo_root,
        now=now,
        state_dir=RENAME_STATE_DIR,
    )


def _emit_denial(agent: str, reason: str) -> None:
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


def _resolve_transcript(payload: dict, cwd: str | None) -> str | None:
    transcript = payload.get("transcript_path") or payload.get("transcriptPath")
    conv_id = payload.get("conversationId") or payload.get("conversation_id")
    if not transcript and conv_id:
        cand = os.path.expanduser(
            f"~/.gemini/antigravity-cli/brain/{conv_id}/.system_generated/logs/transcript.jsonl"
        )
        if os.path.isfile(cand):
            transcript = cand
    sess_id = payload.get("session_id") or payload.get("sessionId")
    if not transcript and sess_id:
        cwd_str = cwd or os.getcwd()
        repo = git_repo_root(cwd_str)
        for path_val in (cwd_str, repo):
            if path_val:
                slug = path_val.replace("/", "-")
                for prefix in (slug, f"-{slug.lstrip('-')}"):
                    cand = os.path.expanduser(
                        f"~/.claude/projects/{prefix}/{sess_id}.jsonl"
                    )
                    if os.path.isfile(cand):
                        return cand
    return transcript


def _handle_edit_renames(payload: dict, edits: list, agy: bool) -> int:
    return evaluate_edit_renames(payload, edits, agy, globals())


def main():
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw)
    except (ValueError, TypeError):
        return 0
    if not isinstance(payload, dict):
        return 0
    agy = isinstance(payload.get("toolCall"), dict)

    edits = file_edits_from_payload(payload)
    if edits:
        return _handle_edit_renames(payload, edits, agy)

    command = shell_command(payload)
    if command is None:
        return 0
    cwd, agent = extract_shell_context(payload, agy)
    reason = evaluate_shell_rules(payload, command, cwd, agent, globals())
    if reason:
        _emit_denial(agent, reason)
    return 0


if __name__ == "__main__":
    sys.exit(main())
