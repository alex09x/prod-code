#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Universal installer and health-checker for prod-code AI agent guard hooks.
Detects installed agent environments (Claude Code, Codex, Antigravity/Gemini),
deploys the canonical guard script, and safely registers PreToolUse hooks.

Usage:
    python3 scripts/hooks/install-hooks.py           # Install/update hooks for all detected agents
    python3 scripts/hooks/install-hooks.py --check   # Check health and status without modifications
    python3 scripts/hooks/install-hooks.py --uninstall # Remove guard hooks from agent configs
"""

import argparse
import json
import os
import shutil
import stat
import sys
from pathlib import Path


CANONICAL_HOOK_DIR = os.path.expanduser("~/.claude/hooks")
CANONICAL_HOOK_NAME = "prod-code-local-build-guard.py"
CANONICAL_HOOK_PATH = os.path.join(CANONICAL_HOOK_DIR, CANONICAL_HOOK_NAME)

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_HOOK_SRC = SCRIPT_DIR / CANONICAL_HOOK_NAME


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
            return json.load(f)
    except Exception:
        return {}


class Provider:
    name: str = ""

    def is_detected(self) -> bool:
        raise NotImplementedError

    def check(self, hook_path: str) -> tuple[bool, str]:
        """Returns (is_ok, details_message)."""
        raise NotImplementedError

    def install(self, hook_path: str) -> str:
        """Installs/updates hook configuration. Returns description of changes."""
        raise NotImplementedError

    def uninstall(self, hook_path: str) -> str:
        """Removes hook configuration. Returns description of changes."""
        raise NotImplementedError


class ClaudeCodeProvider(Provider):
    name = "Claude Code"

    def __init__(self):
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
                        return True, f"Configured ({self.config_file}, matcher: {matcher})"
                    return False, f"Outdated matcher '{matcher}' in {self.config_file} (requires 'Bash|Edit')"
        return False, f"Hook not registered in {self.config_file}"

    def install(self, hook_path: str) -> str:
        data = read_json_safe(self.config_file)
        hooks = data.setdefault("hooks", {})
        pre_tool_use = hooks.setdefault("PreToolUse", [])

        found = False
        changed = False
        for entry in pre_tool_use:
            for h in entry.get("hooks", []):
                if CANONICAL_HOOK_NAME in h.get("command", ""):
                    found = True
                    h["command"] = hook_path
                    if entry.get("matcher") != "Bash|Edit":
                        entry["matcher"] = "Bash|Edit"
                        changed = True

        if not found:
            pre_tool_use.append({
                "matcher": "Bash|Edit",
                "hooks": [
                    {
                        "type": "command",
                        "command": hook_path,
                        "timeout": 5,
                        "statusMessage": "prod-code build guard"
                    }
                ]
            })
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
            e for e in pre_tool_use
            if not any(CANONICAL_HOOK_NAME in h.get("command", "") for h in e.get("hooks", []))
        ]
        if len(new_ptu) != len(pre_tool_use):
            data["hooks"]["PreToolUse"] = new_ptu
            atomic_write_json(self.config_file, data)
            return f"Removed hook from {self.config_file}"
        return "Hook was not registered"


class CodexProvider(Provider):
    name = "Codex"

    def __init__(self):
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
        hooks = data.get("hooks", {}).get("PreToolUse", [])
        for entry in hooks:
            for h in entry.get("hooks", []):
                cmd = h.get("command", "")
                if CANONICAL_HOOK_NAME in cmd:
                    return True, f"Configured ({self.config_file})"
        return False, f"Hook not registered in {self.config_file}"

    def install(self, hook_path: str) -> str:
        data = read_json_safe(self.config_file)
        hooks = data.setdefault("hooks", {})
        pre_tool_use = hooks.setdefault("PreToolUse", [])

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
            pre_tool_use.append({
                "matcher": "",
                "hooks": [
                    {
                        "type": "command",
                        "command": hook_path,
                        "timeout": 5
                    }
                ]
            })
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
            e for e in pre_tool_use
            if not any(CANONICAL_HOOK_NAME in h.get("command", "") for h in e.get("hooks", []))
        ]
        if len(new_ptu) != len(pre_tool_use):
            data["hooks"]["PreToolUse"] = new_ptu
            atomic_write_json(self.config_file, data)
            return f"Removed hook from {self.config_file}"
        return "Hook was not registered"


class AntigravityProvider(Provider):
    name = "Antigravity (Gemini)"

    def __init__(self):
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

        ptu = guard_entry.get("PreToolUse", [])
        for entry in ptu:
            matcher = entry.get("matcher", "")
            for h in entry.get("hooks", []):
                cmd = h.get("command", "")
                if CANONICAL_HOOK_NAME in cmd:
                    if "replace_file_content" in matcher:
                        return True, f"Configured ({self.config_file}, matcher: {matcher})"
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
                    h["command"] = hook_path
                    if entry.get("matcher") != "run_command|replace_file_content":
                        entry["matcher"] = "run_command|replace_file_content"
                        changed = True

        if not found:
            ptu.append({
                "matcher": "run_command|replace_file_content",
                "hooks": [
                    {
                        "type": "command",
                        "command": hook_path,
                        "timeout": 5
                    }
                ]
            })
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


def deploy_canonical_script(src_path: Path, dest_path: str) -> None:
    """Copies source hook script to destination with 0755 permissions."""
    dest = Path(dest_path)
    dest.parent.mkdir(parents=True, exist_ok=True)
    if not src_path.is_file():
        # Fallback: attempt downloading directly from GitHub
        try:
            import urllib.request
            url = f"https://raw.githubusercontent.com/alex09x/prod-code/main/scripts/hooks/{CANONICAL_HOOK_NAME}"
            urllib.request.urlretrieve(url, dest)
        except Exception as e:
            raise FileNotFoundError(f"Source hook script not found at {src_path} and failed to download: {e}")
    else:
        shutil.copy2(src_path, dest)
    dest.chmod(dest.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH | stat.S_IRUSR | stat.S_IRGRP | stat.S_IROTH)


def main() -> int:
    parser = argparse.ArgumentParser(description="Install and manage prod-code guard hooks across AI coding agents.")
    parser.add_argument("--check", action="store_true", help="Inspect and verify configuration without making changes")
    parser.add_argument("--uninstall", action="store_true", help="Remove guard hooks from agent configurations")
    parser.add_argument("--hook-script", type=str, default=None, help=f"Path to source hook script (default: {REPO_HOOK_SRC})")
    args = parser.parse_args()

    hook_src = Path(args.hook_script) if args.hook_script else REPO_HOOK_SRC
    providers: list[Provider] = [ClaudeCodeProvider(), CodexProvider(), AntigravityProvider()]

    print("======================================================================")
    print(" prod-code Guard Hook Installer & Health Checker")
    print("======================================================================")
    print(f"Canonical hook path : {CANONICAL_HOOK_PATH}")
    print(f"Source script path  : {hook_src}")
    print("----------------------------------------------------------------------")

    if args.check:
        print("Checking hook installation across detected agent environments:\n")
        all_ok = True
        script_exists = os.path.isfile(CANONICAL_HOOK_PATH)
        is_exec = script_exists and os.access(CANONICAL_HOOK_PATH, os.X_OK)
        if not script_exists:
            print(f"[-] Canonical hook script : MISSING ({CANONICAL_HOOK_PATH})")
            all_ok = False
        elif not is_exec:
            print(f"[!] Canonical hook script : EXISTS BUT NOT EXECUTABLE ({CANONICAL_HOOK_PATH})")
            all_ok = False
        else:
            print(f"[+] Canonical hook script : OK ({CANONICAL_HOOK_PATH})")

        print("")
        for p in providers:
            detected = p.is_detected()
            if not detected:
                print(f"[-] {p.name:<22} : Not detected")
                continue
            ok, details = p.check(CANONICAL_HOOK_PATH)
            symbol = "[+]" if ok else "[!]"
            print(f"{symbol} {p.name:<22} : {details}")
            if not ok:
                all_ok = False

        print("----------------------------------------------------------------------")
        if all_ok:
            print("Summary: All detected agent environments are healthy and protected.")
            return 0
        else:
            print("Summary: Some agent environments require installation or updates.")
            print("Run without --check to automatically install or update hooks.")
            return 1

    if args.uninstall:
        print("Uninstalling prod-code guard hooks from agent environments:\n")
        for p in providers:
            if not p.is_detected():
                print(f"[-] {p.name:<22} : Not detected (skipped)")
                continue
            res = p.uninstall(CANONICAL_HOOK_PATH)
            print(f"[x] {p.name:<22} : {res}")
        print("\nUninstall completed.")
        return 0

    # Default: Install / Deploy
    print("Deploying canonical hook script and configuring agent providers:\n")
    try:
        deploy_canonical_script(hook_src, CANONICAL_HOOK_PATH)
        print(f"[+] Canonical script   : Deployed and verified executable at {CANONICAL_HOOK_PATH}")
    except Exception as e:
        print(f"[!] Error deploying hook script: {e}", file=sys.stderr)
        return 1

    for p in providers:
        if not p.is_detected():
            print(f"[-] {p.name:<22} : Not detected (skipped)")
            continue
        try:
            res = p.install(CANONICAL_HOOK_PATH)
            print(f"[+] {p.name:<22} : {res}")
        except Exception as e:
            print(f"[!] {p.name:<22} : Failed to configure: {e}", file=sys.stderr)

    print("----------------------------------------------------------------------")
    print("Installation finished. Verifying installation with --check:")
    print("")

    # Run check logic
    all_ok = True
    for p in providers:
        if not p.is_detected():
            continue
        ok, details = p.check(CANONICAL_HOOK_PATH)
        symbol = "[+]" if ok else "[!]"
        print(f"{symbol} {p.name:<22} : {details}")
        if not ok:
            all_ok = False

    if all_ok:
        print("\nStatus: All detected agent providers are fully configured.")
        return 0
    else:
        print("\nStatus: One or more providers failed verification.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
