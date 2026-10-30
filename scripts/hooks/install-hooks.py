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
deploys the canonical guard script and all required guard modules, and safely
registers PreToolUse hooks.

Usage:
    python3 scripts/hooks/install-hooks.py             # Install/update hooks for all detected agents
    python3 scripts/hooks/install-hooks.py --check     # Check health and status without modifications
    python3 scripts/hooks/install-hooks.py --uninstall # Remove guard hooks from agent configs
"""

import argparse
import os
import shutil
import stat
import sys
import urllib.request
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from hook_providers import (
    CANONICAL_HOOK_NAME,
    AntigravityProvider,
    ClaudeCodeProvider,
    CodexProvider,
    Provider,
    atomic_write_json,
    read_json_safe,
)

__all__ = [
    "CANONICAL_HOOK_DIR",
    "CANONICAL_HOOK_NAME",
    "CANONICAL_HOOK_PATH",
    "GUARD_PACKAGE_DIR",
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
]

CANONICAL_HOOK_DIR = os.path.expanduser("~/.claude/hooks")
CANONICAL_HOOK_PATH = os.path.join(CANONICAL_HOOK_DIR, CANONICAL_HOOK_NAME)
REPO_HOOK_SRC = SCRIPT_DIR / CANONICAL_HOOK_NAME
RAW_GITHUB_HOOKS_BASE = "https://raw.githubusercontent.com/alex09x/prod-code/main/scripts/hooks"

GUARD_PACKAGE_DIR = "prod_code_guard"
REQUIRED_GUARD_RELPATHS = (
    f"{GUARD_PACKAGE_DIR}/__init__.py",
    f"{GUARD_PACKAGE_DIR}/shell.py",
    f"{GUARD_PACKAGE_DIR}/grep_guard.py",
    f"{GUARD_PACKAGE_DIR}/sed_guard.py",
    f"{GUARD_PACKAGE_DIR}/shell_python.py",
    f"{GUARD_PACKAGE_DIR}/python_write_guard.py",
    f"{GUARD_PACKAGE_DIR}/rename_guard.py",
)
REQUIRED_GUARD_COMPONENTS = (CANONICAL_HOOK_NAME, *REQUIRED_GUARD_RELPATHS)


def _download_component(url: str, dest_tmp: Path) -> None:
    with urllib.request.urlopen(url, timeout=15) as resp:
        body = resp.read()
    if not body:
        raise ValueError(f"Empty download from {url}")
    dest_tmp.write_bytes(body)


def deploy_canonical_script(
    src_path: Path, dest_path: str, base_url: str = RAW_GITHUB_HOOKS_BASE
) -> None:
    """Deploys the canonical guard entrypoint and all required guard modules atomically."""
    src = Path(src_path)
    src_dir = src.parent
    dest = Path(dest_path)
    dest_dir = dest.parent
    dest_dir.mkdir(parents=True, exist_ok=True)

    manifest: list[tuple[Path, str, Path, bool]] = [(src, CANONICAL_HOOK_NAME, dest, True)]
    for rel in REQUIRED_GUARD_RELPATHS:
        manifest.append((src_dir / rel, rel, dest_dir / rel, False))

    staged: list[tuple[Path, Path, bool]] = []
    suffix = f".tmp.{os.getpid()}_{os.urandom(4).hex()}"
    try:
        for local_src, remote_rel, final_dest, is_exec in manifest:
            final_dest.parent.mkdir(parents=True, exist_ok=True)
            tmp_dest = final_dest.with_name(final_dest.name + suffix)
            if local_src.is_file():
                shutil.copy2(local_src, tmp_dest)
            else:
                url = f"{base_url.rstrip('/')}/{remote_rel}"
                try:
                    _download_component(url, tmp_dest)
                except Exception as exc:
                    raise FileNotFoundError(
                        f"Required hook component missing at {local_src} and failed to download from {url}: {exc}"
                    ) from exc
            staged.append((tmp_dest, final_dest, is_exec))

        for tmp_dest, final_dest, is_exec in staged:
            mode = stat.S_IRUSR | stat.S_IWUSR | stat.S_IRGRP | stat.S_IROTH
            if is_exec:
                mode |= stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            tmp_dest.chmod(mode)
            os.replace(tmp_dest, final_dest)
    except Exception:
        for tmp_dest, _, _ in staged:
            if tmp_dest.exists():
                try:
                    tmp_dest.unlink()
                except OSError:
                    pass
        raise


def check_canonical_hook(hook_path: str) -> tuple[bool, str]:
    """Verifies the canonical guard script and all required guard modules exist and are usable."""
    hook = Path(hook_path)
    if not hook.is_file():
        return False, f"MISSING ({hook_path})"
    if not os.access(hook_path, os.X_OK):
        return False, f"EXISTS BUT NOT EXECUTABLE ({hook_path})"

    hook_dir = hook.parent
    missing = [
        rel
        for rel in REQUIRED_GUARD_RELPATHS
        if not (hook_dir / rel).is_file()
        or not os.access(hook_dir / rel, os.R_OK)
        or (hook_dir / rel).stat().st_size == 0
    ]
    if missing:
        return False, f"MISSING REQUIRED COMPONENTS ({', '.join(missing)}) in {hook_dir}"
    return True, f"OK ({hook_path}, +{len(REQUIRED_GUARD_RELPATHS)} modules)"


def _run_verification(canonical_path: str, providers: list[Provider]) -> bool:
    hook_ok, hook_msg = check_canonical_hook(canonical_path)
    hook_sym = "[+]" if hook_ok else ("[-]" if hook_msg.startswith("MISSING (") else "[!]")
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
    parser.add_argument("--check", action="store_true", help="Inspect and verify configuration without making changes")
    parser.add_argument("--uninstall", action="store_true", help="Remove guard hooks from agent configurations")
    parser.add_argument("--hook-script", type=str, default=None, help=f"Path to source hook script (default: {REPO_HOOK_SRC})")
    parser.add_argument("--canonical-hook", type=str, default=None, help=f"Destination canonical hook path (default: {CANONICAL_HOOK_PATH})")
    args = parser.parse_args(argv)

    hook_src = Path(args.hook_script) if args.hook_script else REPO_HOOK_SRC
    canonical_path = args.canonical_hook or CANONICAL_HOOK_PATH
    active_providers = providers if providers is not None else [ClaudeCodeProvider(), CodexProvider(), AntigravityProvider()]

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
        print(f"[+] Canonical script   : Deployed and verified executable at {canonical_path}")
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
