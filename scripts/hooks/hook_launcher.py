# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Canonical launcher script template, active-version resolution, and bundle health checks."""

import os
from pathlib import Path

LAUNCHER_MARKER = "# prod-code-guard-launcher:v1"
EXEC_MODE = 0o755
FILE_MODE = 0o644
DIR_MODE = 0o755

LAUNCHER_SCRIPT = """#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# SPDX-License-Identifier: MIT OR Apache-2.0
# prod-code-guard-launcher:v1

import sys


def _prod_code_launch() -> None:
    import sys

    mod_name = __name__
    builtins_ref = __builtins__
    orig_path = list(sys.path)
    sys.path[:] = [p for p in sys.path if p not in ("", ".")]
    import os

    dest = os.path.realpath(__file__)
    dest_dir = os.path.dirname(dest)
    dest_name = os.path.basename(dest)
    cwd = os.getcwd()
    versions_dir = os.path.join(dest_dir, f".{dest_name}.versions")
    target = os.readlink(os.path.join(versions_dir, "current"))
    active_dir = os.path.realpath(os.path.join(versions_dir, target))
    if os.path.dirname(active_dir) != os.path.realpath(versions_dir):
        raise RuntimeError(f"Active guard version escapes store: {active_dir}")
    entry = os.path.join(active_dir, dest_name)

    blocked = {
        os.path.abspath(cwd),
        os.path.realpath(cwd),
        os.path.abspath(os.path.dirname(__file__)),
        dest_dir,
    }
    sys.path[:] = [
        p
        for p in orig_path
        if p not in ("", ".")
        and os.path.abspath(p) not in blocked
        and os.path.realpath(p) not in blocked
    ]
    sys.path.insert(0, active_dir)
    cached = {
        name: sys.modules.pop(name)
        for name in list(sys.modules)
        if name == "prod_code_guard" or name.startswith("prod_code_guard.")
    }
    if mod_name == "__main__" and sys.argv:
        sys.argv[0] = entry

    with open(entry, "rb") as fh:
        raw = fh.read()
    if raw.startswith(b"#!") and b"python" not in raw.splitlines()[0].lower() and mod_name == "__main__":
        os.execv(entry, [entry, *sys.argv[1:]])

    code = compile(raw, entry, "exec")
    mod = sys.modules.get(mod_name)
    ns = mod.__dict__ if mod is not None else globals()
    ns.clear()
    ns.update(
        __name__=mod_name,
        __file__=entry,
        __cached__=None,
        __doc__=None,
        __loader__=None,
        __package__=None,
        __spec__=None,
        __builtins__=builtins_ref,
    )
    try:
        exec(code, ns)
    finally:
        if mod_name != "__main__":
            for name in list(sys.modules):
                if name == "prod_code_guard" or name.startswith("prod_code_guard."):
                    del sys.modules[name]
            sys.modules.update(cached)
            sys.path[:] = orig_path


_prod_code_launch()
"""
LAUNCHER_BYTES = LAUNCHER_SCRIPT.encode("utf-8")


def versions_store_dir(hook_path: str | Path) -> Path:
    """Returns the hidden version store directory associated with hook_path."""
    hook = Path(hook_path)
    return hook.parent / f".{hook.name}.versions"


def _has_valid_launcher_bytes(hook: Path) -> bool:
    if hook.is_symlink() or not hook.is_file():
        return False
    try:
        return hook.read_bytes() == LAUNCHER_BYTES
    except OSError:
        return False


def _is_valid_launcher(hook: Path) -> bool:
    return _has_valid_launcher_bytes(hook) and os.access(hook, os.X_OK)


def _is_readable_nonempty_file(path: Path) -> bool:
    try:
        return path.is_file() and os.access(path, os.R_OK) and path.stat().st_size > 0
    except OSError:
        return False


def resolve_active_version_dir(hook_path: str | Path) -> Path | None:
    """Resolves the active immutable version directory for a deployed launcher hook."""
    hook = Path(hook_path)
    if not _has_valid_launcher_bytes(hook):
        return None
    store = versions_store_dir(hook)
    current_link = store / "current"
    if not current_link.is_symlink():
        return None
    try:
        active = (store / os.readlink(current_link)).resolve()
        if active.parent != store.resolve() or not active.is_dir():
            return None
        return active
    except OSError:
        return None


def resolve_active_entrypoint(hook_path: str | Path) -> Path | None:
    """Returns the active version's entrypoint script path, or None if not active."""
    hook = Path(hook_path)
    active = resolve_active_version_dir(hook)
    return (active / hook.name) if active is not None else None


def check_canonical_hook_bundle(
    hook_path: str, required_relpaths: tuple[str, ...]
) -> tuple[bool, str]:
    """Verifies the canonical launcher, active version pointer, entrypoint, and modules."""
    hook = Path(hook_path)
    if hook.is_symlink() and not hook.exists():
        return False, f"BROKEN SYMLINK ({hook_path})"
    if not hook.is_file():
        return False, f"MISSING ({hook_path})"
    if not os.access(hook_path, os.X_OK):
        return False, f"EXISTS BUT NOT EXECUTABLE ({hook_path})"

    if not _has_valid_launcher_bytes(hook):
        missing_flat = [
            rel
            for rel in required_relpaths
            if not _is_readable_nonempty_file(hook.parent / rel)
        ]
        if missing_flat:
            msg = (
                f"LEGACY UNVERSIONED INSTALL; MISSING REQUIRED COMPONENTS "
                f"({', '.join(missing_flat)}) in {hook.parent}"
            )
            return False, msg
        return False, f"LEGACY UNVERSIONED INSTALL ({hook_path})"

    active_dir = resolve_active_version_dir(hook)
    if active_dir is None:
        return (
            False,
            f"BROKEN ACTIVE VERSION POINTER ({versions_store_dir(hook) / 'current'})",
        )

    entry = active_dir / hook.name
    if not _is_readable_nonempty_file(entry):
        return False, f"MISSING ACTIVE ENTRYPOINT ({entry})"
    if not os.access(entry, os.X_OK):
        return False, f"ACTIVE ENTRYPOINT NOT EXECUTABLE ({entry})"

    missing = [
        rel
        for rel in required_relpaths
        if not _is_readable_nonempty_file(active_dir / rel)
    ]
    if missing:
        return (
            False,
            f"MISSING REQUIRED COMPONENTS ({', '.join(missing)}) in {active_dir}",
        )
    return (
        True,
        f"OK ({hook_path} -> {active_dir.name}, +{len(required_relpaths)} modules)",
    )
