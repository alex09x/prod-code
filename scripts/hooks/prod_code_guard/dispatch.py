# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""PreToolUse context normalization, edit-rename evaluation, and shell rule dispatch."""

import json
import os
import time
from collections.abc import MutableMapping
from typing import Any


def extract_shell_context(payload: dict, agy: bool) -> tuple[str, str]:
    """Extracts (cwd, agent) from a PreToolUse shell tool payload."""
    if agy:
        args = payload["toolCall"].get("args") or {}
        paths = payload.get("workspacePaths") or []
        cwd = args.get("Cwd") or (paths[0] if paths else os.getcwd())
        return cwd, "antigravity"

    tool_in = (
        payload.get("tool_input") if isinstance(payload.get("tool_input"), dict) else {}
    )
    cwd = (
        tool_in.get("workdir")
        or tool_in.get("cwd")
        or payload.get("cwd")
        or os.getcwd()
    )
    agent = (
        "gemini"
        if payload.get("hook_event_name") == "BeforeTool"
        else payload.get("tool_name", "")
    )
    return cwd, agent


def evaluate_edit_renames(
    payload: dict, edits: list, agy: bool, ns: MutableMapping[str, Any]
) -> int:
    """Evaluates file-edit tool payloads for manual multi-file symbol renames."""
    if agy:
        paths = payload.get("workspacePaths") or []
        cwd = paths[0] if paths else os.getcwd()
        agent = "antigravity"
        conv_id = payload.get("conversationId") or "default"
    else:
        tool_in = (
            payload.get("tool_input")
            if isinstance(payload.get("tool_input"), dict)
            else {}
        )
        cwd = (
            tool_in.get("workdir")
            or tool_in.get("cwd")
            or payload.get("cwd")
            or os.getcwd()
        )
        agent = (
            "gemini"
            if payload.get("hook_event_name") == "BeforeTool"
            else payload.get("tool_name", "")
        )
        conv_id = (
            payload.get("conversation_id") or payload.get("session_id") or "default"
        )

    git_repo_root = ns["git_repo_root"]
    code_ext = ns["CODE_EXT"]
    single_symbol_rename = ns["single_symbol_rename"]
    log = ns["log"]
    check_multi_file_rename = ns["check_multi_file_rename"]
    emit_denial = ns["_emit_denial"]

    for target_file, old_content, replacement, override in edits:
        resolved_file = (
            target_file
            if os.path.isabs(target_file)
            else os.path.join(cwd, target_file)
        )
        repo_root = git_repo_root(resolved_file) or git_repo_root(cwd)
        if not (
            resolved_file and code_ext.search(resolved_file) and repo_root is not None
        ):
            continue
        sym_pair = single_symbol_rename(old_content, replacement)
        if not sym_pair:
            continue
        old_sym, new_sym = sym_pair
        if override:
            log(
                {
                    "ts": int(time.time()),
                    "agent": agent,
                    "rule": "multi-file-rename",
                    "decision": "override",
                    "cwd": cwd,
                    "symbol": old_sym,
                    "file": resolved_file,
                }
            )
            continue
        is_multi, prev_file = check_multi_file_rename(
            conv_id, resolved_file, old_sym, new_sym, repo_root=repo_root
        )
        if is_multi:
            log(
                {
                    "ts": int(time.time()),
                    "agent": agent,
                    "rule": "multi-file-rename",
                    "decision": "deny",
                    "cwd": cwd,
                    "symbol": old_sym,
                    "file": resolved_file,
                    "prev_file": prev_file,
                }
            )
            prev_name, cur_name = (
                os.path.basename(prev_file),
                os.path.basename(resolved_file),
            )
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
            emit_denial(agent, reason)
            return 0
        log(
            {
                "ts": int(time.time()),
                "agent": agent,
                "rule": "multi-file-rename",
                "decision": "first-file-tracked",
                "cwd": cwd,
                "symbol": old_sym,
                "file": resolved_file,
            }
        )
    return 0


def _evaluate_grep_rules(
    payload: dict, command: str, cwd: str, agent: str, ns: MutableMapping[str, Any]
) -> str | None:
    log = ns["log"]
    symbol_grep = ns["symbol_grep"]
    ripgrep_pattern = ns["ripgrep_pattern"]
    transcript = ns["_resolve_transcript"](payload, cwd)
    found = symbol_grep(command, cwd)
    if found is None and "PROD_CODE_GREP=1" in command:
        again = symbol_grep(command.replace("PROD_CODE_GREP=1", ""), cwd)
        if again:
            sym = again[0]
            if not ns["asked_prod_code"](transcript, sym, agent=agent):
                log(
                    {
                        "ts": int(time.time()),
                        "agent": agent,
                        "rule": "symbol-grep",
                        "decision": "override-refused",
                        "cwd": cwd,
                        "symbol": sym,
                        "cmd": command[:300],
                    }
                )
                return (
                    f"Grep for symbol `{sym}` is blocked. Ask prod-code first: "
                    f'code_definition {{symbol: "{sym}"}}, code_symbols {{query: "{sym}"}}, '
                    f'or code_grep {{pattern: "{sym}"}} (`prod-code grep "{sym}"`).'
                )
            if not ns["reported_prod_code_issue"](transcript, sym, agent=agent):
                log(
                    {
                        "ts": int(time.time()),
                        "agent": agent,
                        "rule": "symbol-grep",
                        "decision": "override-refused",
                        "cwd": cwd,
                        "symbol": sym,
                        "cmd": command[:300],
                    }
                )
                return (
                    f"Grep for symbol `{sym}` is blocked. If prod-code failed to resolve `{sym}`, "
                    f'you must report the bug first: code_report_issue {{title: "Symbol not found: {sym}", body: "..."}}.'
                )
            log(
                {
                    "ts": int(time.time()),
                    "agent": agent,
                    "rule": "symbol-grep",
                    "decision": "override",
                    "cwd": cwd,
                    "symbol": sym,
                    "cmd": command[:300],
                }
            )
    if found:
        sym, pat = found
        log(
            {
                "ts": int(time.time()),
                "agent": agent,
                "rule": "symbol-grep",
                "decision": "deny",
                "cwd": cwd,
                "symbol": sym,
                "pattern": pat,
                "cmd": command[:300],
            }
        )
        rg = ripgrep_pattern(command, cwd, pat)
        return (
            f"Grep for code symbol `{sym}` is blocked (`{pat}`). Use prod-code AST tools: "
            f'code_definition {{symbol: "{sym}"}}, code_references {{symbol: "{sym}"}}, or code_symbols {{query: "{sym}"}} '
            f'(or for literal/regex text search, use code_grep {{pattern: "{rg}"}} / `prod-code grep "{rg}"`). '
            f'If prod-code returns empty or errors, file a bug: code_report_issue {{title: "Symbol not found: {sym}", body: "..."}}.'
        )

    found_text = ns["text_grep"](command, cwd)
    if found_text is not None:
        pat, target_path = found_text
        log(
            {
                "ts": int(time.time()),
                "agent": agent,
                "rule": "text-grep",
                "decision": "deny",
                "cwd": cwd,
                "pattern": pat,
                "cmd": command[:300],
            }
        )
        rg = ripgrep_pattern(command, cwd, pat)
        mcp_call = (
            f'code_grep {{pattern: "{rg}", path: "{target_path}"}}'
            if target_path
            else f'code_grep {{pattern: "{rg}"}}'
        )
        cli_call = (
            f'prod-code grep "{rg}" {target_path}'
            if target_path
            else f'prod-code grep "{rg}"'
        )
        return (
            f"Grep for text pattern `{pat}` over source files is blocked. "
            f"Use prod-code literal/regex text search instead: {mcp_call} (CLI: `{cli_call}`). "
            f"It runs ripgrep on the build node and returns file:line:col with the enclosing AST symbol (`[in Type::method]`). "
            f"For AST symbol lookups use code_symbols / code_definition / code_references; for concept search use code_search."
        )
    return None


def evaluate_shell_rules(
    payload: dict, command: str, cwd: str, agent: str, ns: MutableMapping[str, Any]
) -> str | None:
    """Evaluates local-build, scripted-write, grep, and source-sed rules in order."""
    log = ns["log"]
    reason = ns["verdict"](command)
    if reason:
        log(
            {
                "ts": int(time.time()),
                "agent": agent,
                "rule": "local-build",
                "decision": "deny",
                "cwd": cwd,
                "cmd": command[:300],
            }
        )
        return reason

    write_target = ns["scripted_source_write_target"](command, cwd)
    if write_target:
        log(
            {
                "ts": int(time.time()),
                "agent": agent,
                "rule": "scripted-source-write",
                "decision": "deny",
                "cwd": cwd,
                "file": write_target,
                "cmd": command[:300],
            }
        )
        return (
            f"Inline Python rewrites of repository source files are blocked (`{write_target}`). "
            f"Use a native contextual edit tool (`Edit`, `replace_file_content`, or `apply_patch`) "
            f"so surrounding lines are verified, or use a `prod-code` refactoring tool "
            f"(`code_rename`, `code_assist`, `code_change_signature`) for semantic refactors."
        )

    reason = _evaluate_grep_rules(payload, command, cwd, agent, ns)
    if reason:
        return reason

    source_target = ns["sed_source_target"](command, cwd)
    if source_target:
        if ns["source_sed_fallback_requested"](command, cwd):
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
            return None
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
        return (
            f"Direct `sed` reads of source-code files are blocked (`{source_target}`). "
            f"Use code_outline {{path: {json.dumps(source_target)}}} to inspect file structure, "
            f"code_definition for a named declaration, or code_grep for literal text. "
            f"If prod-code is unavailable, prefix this single read with "
            f"`PROD_CODE_SOURCE_FALLBACK=1`; fallback use is logged. Report a persistent prod-code outage."
        )
    return None
