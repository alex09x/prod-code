# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Agent provider configuration adapters for Claude Code, Codex, and Antigravity."""

import json
import os
import shutil

CANONICAL_HOOK_NAME = "prod-code-local-build-guard.py"


def atomic_write_json(file_path: str, data: dict) -> None:
    """Safely and atomically writes JSON data to file_path."""
    os.makedirs(os.path.dirname(os.path.abspath(file_path)), exist_ok=True)
    temp_path = f"{file_path}.tmp.{os.getpid()}_{os.urandom(4).hex()}"
    with open(temp_path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    os.replace(temp_path, file_path)


def read_json_safe(file_path: str) -> dict:
    """Reads JSON from file_path, returning empty dict if missing or malformed."""
    if not os.path.isfile(file_path):
        return {}
    try:
        with open(file_path, "r", encoding="utf-8") as f:
            loaded = json.load(f)
            return loaded if isinstance(loaded, dict) else {}
    except (OSError, ValueError):
        return {}


class Provider:
    name: str = ""

    def is_detected(self) -> bool:
        raise NotImplementedError

    def check(self, hook_path: str) -> tuple[bool, str]:
        raise NotImplementedError

    def install(self, hook_path: str) -> str:
        raise NotImplementedError

    def uninstall(self, hook_path: str) -> str:
        raise NotImplementedError


class ClaudeCodeProvider(Provider):
    name = "Claude Code"

    def __init__(self) -> None:
        self.home_dir = os.path.expanduser("~/.claude")
        self.config_file = os.path.join(self.home_dir, "settings.json")

    def is_detected(self) -> bool:
        return os.path.isdir(self.home_dir) or shutil.which("claude") is not None

    def check(self, hook_path: str) -> tuple[bool, str]:
        if not self.is_detected():
            return True, "Not installed (skipped)"
        if not os.path.isfile(self.config_file):
            return False, f"Missing settings file: {self.config_file}"

        data = read_json_safe(self.config_file)
        hooks = data.get("hooks", {}).get("PreToolUse", [])
        for entry in hooks:
            matcher = entry.get("matcher", "")
            for h in entry.get("hooks", []):
                cmd = h.get("command", "")
                if CANONICAL_HOOK_NAME in cmd:
                    if "Edit" in matcher:
                        return (
                            True,
                            f"Configured ({self.config_file}, matcher: {matcher})",
                        )
                    return (
                        False,
                        f"Outdated matcher '{matcher}' in {self.config_file} (requires 'Bash|Edit')",
                    )
        return False, f"Hook not registered in {self.config_file}"

    def install(self, hook_path: str) -> str:
        data = read_json_safe(self.config_file)
        pre_tool_use = data.setdefault("hooks", {}).setdefault("PreToolUse", [])

        found = False
        changed = False
        for entry in pre_tool_use:
            for h in entry.get("hooks", []):
                if CANONICAL_HOOK_NAME in h.get("command", ""):
                    found = True
                    if h.get("command") != hook_path:
                        h["command"] = hook_path
                        changed = True
                    if entry.get("matcher") != "Bash|Edit":
                        entry["matcher"] = "Bash|Edit"
                        changed = True

        if not found:
            pre_tool_use.append(
                {
                    "matcher": "Bash|Edit",
                    "hooks": [
                        {
                            "type": "command",
                            "command": hook_path,
                            "timeout": 5,
                            "statusMessage": "prod-code build guard",
                        }
                    ],
                }
            )
            changed = True

        if changed or not os.path.exists(self.config_file):
            atomic_write_json(self.config_file, data)
            return f"Updated {self.config_file} (matcher: Bash|Edit)"
        return f"Already up-to-date in {self.config_file}"

    def uninstall(self, hook_path: str) -> str:
        if not os.path.isfile(self.config_file):
            return "No configuration file found"
        data = read_json_safe(self.config_file)
        pre_tool_use = data.get("hooks", {}).get("PreToolUse", [])
        new_ptu = [
            e
            for e in pre_tool_use
            if not any(
                CANONICAL_HOOK_NAME in h.get("command", "") for h in e.get("hooks", [])
            )
        ]
        if len(new_ptu) != len(pre_tool_use):
            data["hooks"]["PreToolUse"] = new_ptu
            atomic_write_json(self.config_file, data)
            return f"Removed hook from {self.config_file}"
        return "Hook was not registered"


class CodexProvider(Provider):
    name = "Codex"

    def __init__(self) -> None:
        self.home_dir = os.path.expanduser("~/.codex")
        self.config_file = os.path.join(self.home_dir, "hooks.json")

    def is_detected(self) -> bool:
        return os.path.isdir(self.home_dir) or shutil.which("codex") is not None

    def check(self, hook_path: str) -> tuple[bool, str]:
        if not self.is_detected():
            return True, "Not installed (skipped)"
        if not os.path.isfile(self.config_file):
            return False, f"Missing hooks file: {self.config_file}"

        data = read_json_safe(self.config_file)
        for entry in data.get("hooks", {}).get("PreToolUse", []):
            for h in entry.get("hooks", []):
                if CANONICAL_HOOK_NAME in h.get("command", ""):
                    return True, f"Configured ({self.config_file})"
        return False, f"Hook not registered in {self.config_file}"

    def install(self, hook_path: str) -> str:
        data = read_json_safe(self.config_file)
        pre_tool_use = data.setdefault("hooks", {}).setdefault("PreToolUse", [])

        found = False
        changed = False
        for entry in pre_tool_use:
            for h in entry.get("hooks", []):
                if CANONICAL_HOOK_NAME in h.get("command", ""):
                    found = True
                    if h.get("command") != hook_path:
                        h["command"] = hook_path
                        changed = True

        if not found:
            pre_tool_use.append(
                {
                    "matcher": "",
                    "hooks": [{"type": "command", "command": hook_path, "timeout": 5}],
                }
            )
            changed = True

        if changed or not os.path.exists(self.config_file):
            atomic_write_json(self.config_file, data)
            return f"Updated {self.config_file}"
        return f"Already up-to-date in {self.config_file}"

    def uninstall(self, hook_path: str) -> str:
        if not os.path.isfile(self.config_file):
            return "No configuration file found"
        data = read_json_safe(self.config_file)
        pre_tool_use = data.get("hooks", {}).get("PreToolUse", [])
        new_ptu = [
            e
            for e in pre_tool_use
            if not any(
                CANONICAL_HOOK_NAME in h.get("command", "") for h in e.get("hooks", [])
            )
        ]
        if len(new_ptu) != len(pre_tool_use):
            data["hooks"]["PreToolUse"] = new_ptu
            atomic_write_json(self.config_file, data)
            return f"Removed hook from {self.config_file}"
        return "Hook was not registered"


class AntigravityProvider(Provider):
    name = "Antigravity (Gemini)"

    def __init__(self) -> None:
        self.home_dir = os.path.expanduser("~/.gemini")
        self.config_file = os.path.join(self.home_dir, "config", "hooks.json")

    def is_detected(self) -> bool:
        return (
            os.path.isdir(self.home_dir)
            or shutil.which("agy") is not None
            or shutil.which("gemini") is not None
        )

    def check(self, hook_path: str) -> tuple[bool, str]:
        if not self.is_detected():
            return True, "Not installed (skipped)"
        if not os.path.isfile(self.config_file):
            return False, f"Missing hooks file: {self.config_file}"

        data = read_json_safe(self.config_file)
        guard_entry = data.get("prod-code-guard", {})
        if not guard_entry.get("enabled", False):
            return False, f"prod-code-guard disabled in {self.config_file}"

        for entry in guard_entry.get("PreToolUse", []):
            matcher = entry.get("matcher", "")
            for h in entry.get("hooks", []):
                if CANONICAL_HOOK_NAME in h.get("command", ""):
                    if "replace_file_content" in matcher:
                        return (
                            True,
                            f"Configured ({self.config_file}, matcher: {matcher})",
                        )
                    return False, f"Outdated matcher '{matcher}' in {self.config_file}"
        return False, f"prod-code-guard not registered in {self.config_file}"

    def install(self, hook_path: str) -> str:
        data = read_json_safe(self.config_file)
        guard_entry = data.setdefault("prod-code-guard", {})
        guard_entry["enabled"] = True
        ptu = guard_entry.setdefault("PreToolUse", [])

        found = False
        changed = False
        for entry in ptu:
            for h in entry.get("hooks", []):
                if CANONICAL_HOOK_NAME in h.get("command", ""):
                    found = True
                    if h.get("command") != hook_path:
                        h["command"] = hook_path
                        changed = True
                    if entry.get("matcher") != "run_command|replace_file_content":
                        entry["matcher"] = "run_command|replace_file_content"
                        changed = True

        if not found:
            ptu.append(
                {
                    "matcher": "run_command|replace_file_content",
                    "hooks": [{"type": "command", "command": hook_path, "timeout": 5}],
                }
            )
            changed = True

        if changed or not os.path.exists(self.config_file):
            atomic_write_json(self.config_file, data)
            return f"Updated {self.config_file} (matcher: run_command|replace_file_content)"
        return f"Already up-to-date in {self.config_file}"

    def uninstall(self, hook_path: str) -> str:
        if not os.path.isfile(self.config_file):
            return "No configuration file found"
        data = read_json_safe(self.config_file)
        if "prod-code-guard" in data:
            del data["prod-code-guard"]
            atomic_write_json(self.config_file, data)
            return f"Removed prod-code-guard from {self.config_file}"
        return "prod-code-guard was not registered"
