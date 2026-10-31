# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Agent PreToolUse payload and patch extraction across Claude Code, Codex, Gemini, and Antigravity."""

import os
import shlex

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
