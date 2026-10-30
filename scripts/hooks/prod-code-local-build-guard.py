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
import time
from pathlib import Path

_HOOK_DIR = str(Path(__file__).resolve().parent)
if _HOOK_DIR not in sys.path:
    sys.path.insert(0, _HOOK_DIR)

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


def check_multi_file_rename(conv_id, file_path, old_sym, new_sym, repo_root=None, now=None):
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
                    cand = os.path.expanduser(f"~/.claude/projects/{prefix}/{sess_id}.jsonl")
                    if os.path.isfile(cand):
                        return cand
    return transcript


def _handle_edit_renames(payload: dict, edits: list, agy: bool) -> int:
    if agy:
        paths = payload.get("workspacePaths") or []
        cwd = paths[0] if paths else os.getcwd()
        agent = "antigravity"
        conv_id = payload.get("conversationId") or "default"
    else:
        tool_in = payload.get("tool_input") if isinstance(payload.get("tool_input"), dict) else {}
        cwd = tool_in.get("workdir") or tool_in.get("cwd") or payload.get("cwd") or os.getcwd()
        agent = "gemini" if payload.get("hook_event_name") == "BeforeTool" else payload.get("tool_name", "")
        conv_id = payload.get("conversation_id") or payload.get("session_id") or "default"

    for target_file, old_content, replacement, override in edits:
        resolved_file = target_file if os.path.isabs(target_file) else os.path.join(cwd, target_file)
        repo_root = git_repo_root(resolved_file) or git_repo_root(cwd)
        if not (resolved_file and CODE_EXT.search(resolved_file) and repo_root is not None):
            continue
        sym_pair = single_symbol_rename(old_content, replacement)
        if not sym_pair:
            continue
        old_sym, new_sym = sym_pair
        if override:
            log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "override", "cwd": cwd, "symbol": old_sym, "file": resolved_file})
            continue
        is_multi, prev_file = check_multi_file_rename(conv_id, resolved_file, old_sym, new_sym, repo_root=repo_root)
        if is_multi:
            log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "deny", "cwd": cwd, "symbol": old_sym, "file": resolved_file, "prev_file": prev_file})
            prev_name, cur_name = os.path.basename(prev_file), os.path.basename(resolved_file)
            reason = (
                f"Multi-file symbol rename detected: replacing `{old_sym}` with `{new_sym}` across files "
                f"(`{prev_name}` and `{cur_name}`). Do not edit files manually across the workspace. "
                f"Use the AST refactoring tool instead: "
                f'code_rename {{path: "{cur_name}", line: ..., character: ..., new_name: "{new_sym}"}} '
                f"(CLI: prod-code rename <file> <line> <col> {new_sym}). "
                f"It updates all declarations, references, and imports across the entire workspace atomically and safely. "
                f"If this is not an automated symbol rename, include PROD_CODE_MANUAL_RENAME=1 in your edit description/explanation "
                f"or set PROD_CODE_MANUAL_RENAME=1 in the environment."
            )
            _emit_denial(agent, reason)
            return 0
        log({"ts": int(time.time()), "agent": agent, "rule": "multi-file-rename", "decision": "first-file-tracked", "cwd": cwd, "symbol": old_sym, "file": resolved_file})
    return 0


def main():
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw)
    except (ValueError, TypeError):
        return 0
    agy = isinstance(payload.get("toolCall"), dict)

    edits = file_edits_from_payload(payload)
    if edits:
        return _handle_edit_renames(payload, edits, agy)

    command = shell_command(payload)
    if command is None:
        return 0
    if agy:
        args = payload["toolCall"].get("args") or {}
        paths = payload.get("workspacePaths") or []
        cwd = args.get("Cwd") or (paths[0] if paths else os.getcwd())
        agent = "antigravity"
    else:
        tool_in = payload.get("tool_input") if isinstance(payload.get("tool_input"), dict) else {}
        cwd = tool_in.get("workdir") or tool_in.get("cwd") or payload.get("cwd") or os.getcwd()
        agent = "gemini" if payload.get("hook_event_name") == "BeforeTool" else payload.get("tool_name", "")

    reason = verdict(command)
    if reason:
        log({"ts": int(time.time()), "agent": agent, "rule": "local-build", "decision": "deny", "cwd": cwd, "cmd": command[:300]})

    if not reason:
        write_target = scripted_source_write_target(command, cwd)
        if write_target:
            log({"ts": int(time.time()), "agent": agent, "rule": "scripted-source-write", "decision": "deny", "cwd": cwd, "file": write_target, "cmd": command[:300]})
            reason = (
                f"Inline Python rewrites of repository source files are blocked (`{write_target}`). "
                f"Use a native contextual edit tool (`Edit`, `replace_file_content`, or `apply_patch`) "
                f"so surrounding lines are verified, or use a `prod-code` refactoring tool "
                f"(`code_rename`, `code_assist`, `code_change_signature`) for semantic refactors."
            )

    if not reason:
        transcript = _resolve_transcript(payload, cwd)
        found = symbol_grep(command, cwd)
        if found is None and "PROD_CODE_GREP=1" in command:
            again = symbol_grep(command.replace("PROD_CODE_GREP=1", ""), cwd)
            if again:
                sym = again[0]
                if not asked_prod_code(transcript, sym, agent=agent):
                    log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override-refused", "cwd": cwd, "symbol": sym, "cmd": command[:300]})
                    reason = (
                        f"Grep for symbol `{sym}` is blocked. Ask prod-code first: "
                        f'code_definition {{symbol: "{sym}"}}, code_symbols {{query: "{sym}"}}, '
                        f'or code_grep {{pattern: "{sym}"}} (`prod-code grep "{sym}"`).'
                    )
                elif not reported_prod_code_issue(transcript, sym, agent=agent):
                    log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override-refused", "cwd": cwd, "symbol": sym, "cmd": command[:300]})
                    reason = (
                        f"Grep for symbol `{sym}` is blocked. If prod-code failed to resolve `{sym}`, "
                        f'you must report the bug first: code_report_issue {{title: "Symbol not found: {sym}", body: "..."}}.'
                    )
                else:
                    log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "override", "cwd": cwd, "symbol": sym, "cmd": command[:300]})
        if found:
            sym, pat = found
            log({"ts": int(time.time()), "agent": agent, "rule": "symbol-grep", "decision": "deny", "cwd": cwd, "symbol": sym, "pattern": pat, "cmd": command[:300]})
            rg = ripgrep_pattern(command, cwd, pat)
            reason = (
                f"Grep for code symbol `{sym}` is blocked (`{pat}`). Use prod-code AST tools: "
                f'code_definition {{symbol: "{sym}"}}, code_references {{symbol: "{sym}"}}, or code_symbols {{query: "{sym}"}} '
                f'(or for literal/regex text search, use code_grep {{pattern: "{rg}"}} / `prod-code grep "{rg}"`). '
                f'If prod-code returns empty or errors, file a bug: code_report_issue {{title: "Symbol not found: {sym}", body: "..."}}.'
            )
        elif not reason:
            found_text = text_grep(command, cwd)
            if found_text is not None:
                pat, target_path = found_text
                log({"ts": int(time.time()), "agent": agent, "rule": "text-grep", "decision": "deny", "cwd": cwd, "pattern": pat, "cmd": command[:300]})
                rg = ripgrep_pattern(command, cwd, pat)
                mcp_call = f'code_grep {{pattern: "{rg}", path: "{target_path}"}}' if target_path else f'code_grep {{pattern: "{rg}"}}'
                cli_call = f'prod-code grep "{rg}" {target_path}' if target_path else f'prod-code grep "{rg}"'
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
                log({"ts": int(time.time()), "agent": agent, "rule": "source-sed", "decision": "override", "cwd": cwd, "file": source_target, "cmd": command[:300]})
            else:
                log({"ts": int(time.time()), "agent": agent, "rule": "source-sed", "decision": "deny", "cwd": cwd, "file": source_target, "cmd": command[:300]})
                reason = (
                    f"Direct `sed` reads of source-code files are blocked (`{source_target}`). "
                    f"Use code_outline {{path: {json.dumps(source_target)}}} to inspect file structure, "
                    f"code_definition for a named declaration, or code_grep for literal text. "
                    f"If prod-code is unavailable, prefix this single read with "
                    f"`PROD_CODE_SOURCE_FALLBACK=1`; fallback use is logged. Report a persistent prod-code outage."
                )

    if reason:
        _emit_denial(agent, reason)
    return 0


if __name__ == "__main__":
    sys.exit(main())
