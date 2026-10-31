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
stages a complete versioned guard bundle with atomic pointer activation, and safely
registers PreToolUse hooks.

Usage:
    python3 scripts/hooks/install-hooks.py             # Install/update hooks for all detected agents
    python3 scripts/hooks/install-hooks.py --check     # Check health and status without modifications
    python3 scripts/hooks/install-hooks.py --uninstall # Remove guard hooks from agent configs
"""

import sys

if __name__ == "__main__":
    import posix as _posix

    _cwd_stat = _posix.stat(".")

    def _is_workdir(path: str) -> bool:
        if path in ("", "."):
            return True
        try:
            info = _posix.stat(path)
            return (info.st_dev, info.st_ino) == (_cwd_stat.st_dev, _cwd_stat.st_ino)
        except OSError:
            return False

    sys.path[:] = [p for p in sys.path if not _is_workdir(p)]

import argparse
import importlib.util
import os
from pathlib import Path
from types import ModuleType

if __name__ == "__main__":
    _cwd_abs = os.path.abspath(os.getcwd())
    _cwd_real = os.path.realpath(os.getcwd())
    sys.path[:] = [
        p
        for p in sys.path
        if p not in ("", ".")
        and os.path.abspath(p) != _cwd_abs
        and os.path.realpath(p) != _cwd_real
    ]

SCRIPT_DIR = Path(__file__).resolve().parent


def _load_sibling_module(module_name: str, filename: str) -> ModuleType:
    module_path = SCRIPT_DIR / filename
    spec = importlib.util.spec_from_file_location(module_name, str(module_path))
    if spec is None or spec.loader is None:
        raise ImportError(f"Unable to load installer module at {module_path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_providers_mod = _load_sibling_module("_prod_code_hook_providers", "hook_providers.py")
_deploy_mod = _load_sibling_module("_prod_code_hook_deploy", "hook_deploy.py")

CANONICAL_HOOK_NAME = _providers_mod.CANONICAL_HOOK_NAME
AntigravityProvider = _providers_mod.AntigravityProvider
ClaudeCodeProvider = _providers_mod.ClaudeCodeProvider
CodexProvider = _providers_mod.CodexProvider
Provider = _providers_mod.Provider
atomic_write_json = _providers_mod.atomic_write_json
read_json_safe = _providers_mod.read_json_safe

GUARD_PACKAGE_DIR = _deploy_mod.GUARD_PACKAGE_DIR
LAUNCHER_MARKER = _deploy_mod.LAUNCHER_MARKER
RAW_GITHUB_HOOKS_BASE = _deploy_mod.RAW_GITHUB_HOOKS_BASE
REQUIRED_GUARD_COMPONENTS = _deploy_mod.REQUIRED_GUARD_COMPONENTS
REQUIRED_GUARD_RELPATHS = _deploy_mod.REQUIRED_GUARD_RELPATHS
check_canonical_hook = _deploy_mod.check_canonical_hook
deploy_canonical_script = _deploy_mod.deploy_canonical_script
resolve_active_entrypoint = _deploy_mod.resolve_active_entrypoint
resolve_active_version_dir = _deploy_mod.resolve_active_version_dir
versions_store_dir = _deploy_mod.versions_store_dir

__all__ = [
    "CANONICAL_HOOK_DIR",
    "CANONICAL_HOOK_NAME",
    "CANONICAL_HOOK_PATH",
    "GUARD_PACKAGE_DIR",
    "LAUNCHER_MARKER",
    "RAW_GITHUB_HOOKS_BASE",
    "REPO_HOOK_SRC",
    "REQUIRED_GUARD_COMPONENTS",
    "REQUIRED_GUARD_RELPATHS",
    "AntigravityProvider",
    "ClaudeCodeProvider",
    "CodexProvider",
    "Provider",
    "atomic_write_json",
    "check_canonical_hook",
    "deploy_canonical_script",
    "main",
    "read_json_safe",
    "resolve_active_entrypoint",
    "resolve_active_version_dir",
    "versions_store_dir",
]

CANONICAL_HOOK_DIR = os.path.expanduser("~/.claude/hooks")
CANONICAL_HOOK_PATH = os.path.join(CANONICAL_HOOK_DIR, CANONICAL_HOOK_NAME)
REPO_HOOK_SRC = SCRIPT_DIR / CANONICAL_HOOK_NAME


def _run_verification(canonical_path: str, providers: list[Provider]) -> bool:
    hook_ok, hook_msg = check_canonical_hook(canonical_path)
    hook_sym = (
        "[+]" if hook_ok else ("[-]" if hook_msg.startswith("MISSING (") else "[!]")
    )
    print(f"{hook_sym} Canonical hook script : {hook_msg}")
    all_ok = hook_ok

    print()
    for p in providers:
        if not p.is_detected():
            print(f"[-] {p.name:<22} : Not detected")
            continue
        ok, details = p.check(canonical_path)
        symbol = "[+]" if ok else "[!]"
        print(f"{symbol} {p.name:<22} : {details}")
        if not ok:
            all_ok = False
    return all_ok


def main(argv: list[str] | None = None, providers: list[Provider] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Install and manage prod-code guard hooks across AI coding agents."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Inspect and verify configuration without making changes",
    )
    parser.add_argument(
        "--uninstall",
        action="store_true",
        help="Remove guard hooks from agent configurations",
    )
    parser.add_argument(
        "--hook-script",
        type=str,
        default=None,
        help=f"Path to source hook script (default: {REPO_HOOK_SRC})",
    )
    parser.add_argument(
        "--canonical-hook",
        type=str,
        default=None,
        help=f"Destination canonical hook path (default: {CANONICAL_HOOK_PATH})",
    )
    args = parser.parse_args(argv)

    hook_src = Path(args.hook_script) if args.hook_script else REPO_HOOK_SRC
    canonical_path = args.canonical_hook or CANONICAL_HOOK_PATH
    active_providers = (
        providers
        if providers is not None
        else [ClaudeCodeProvider(), CodexProvider(), AntigravityProvider()]
    )

    print("======================================================================")
    print(" prod-code Guard Hook Installer & Health Checker")
    print("======================================================================")
    print(f"Canonical hook path : {canonical_path}")
    print(f"Source script path  : {hook_src}")
    print("----------------------------------------------------------------------")

    if args.check:
        print("Checking hook installation across detected agent environments:\n")
        all_ok = _run_verification(canonical_path, active_providers)
        print("----------------------------------------------------------------------")
        if all_ok:
            print("Summary: All detected agent environments are healthy and protected.")
            return 0
        print("Summary: Some agent environments require installation or updates.")
        print("Run without --check to automatically install or update hooks.")
        return 1

    if args.uninstall:
        print("Uninstalling prod-code guard hooks from agent environments:\n")
        for p in active_providers:
            if not p.is_detected():
                print(f"[-] {p.name:<22} : Not detected (skipped)")
                continue
            print(f"[x] {p.name:<22} : {p.uninstall(canonical_path)}")
        print("\nUninstall completed.")
        return 0

    print("Deploying canonical hook script and configuring agent providers:\n")
    try:
        deploy_canonical_script(hook_src, canonical_path)
        print(
            f"[+] Canonical script   : Deployed and verified executable at {canonical_path}"
        )
    except (OSError, ValueError) as e:
        print(f"[!] Error deploying hook script: {e}", file=sys.stderr)
        return 1

    install_failed = False
    for p in active_providers:
        if not p.is_detected():
            print(f"[-] {p.name:<22} : Not detected (skipped)")
            continue
        try:
            print(f"[+] {p.name:<22} : {p.install(canonical_path)}")
        except (OSError, ValueError) as e:
            install_failed = True
            print(f"[!] {p.name:<22} : Failed to configure: {e}", file=sys.stderr)

    print("----------------------------------------------------------------------")
    print("Installation finished. Verifying installation with --check:\n")
    all_ok = _run_verification(canonical_path, active_providers) and not install_failed
    if all_ok:
        print("\nStatus: All detected agent providers are fully configured.")
        return 0
    print("\nStatus: One or more providers or required components failed verification.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
