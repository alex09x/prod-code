# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Versioned staging and atomic activation for the canonical prod-code guard hook."""

import fcntl
import json
import os
import re
import shutil
import time
import urllib.request
from pathlib import Path

CANONICAL_HOOK_NAME = "prod-code-local-build-guard.py"
RAW_GITHUB_HOOKS_BASE = (
    "https://raw.githubusercontent.com/alex09x/prod-code/main/scripts/hooks"
)

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


def _immutable_remote_base(base_url: str) -> str:
    if base_url.rstrip("/") != RAW_GITHUB_HOOKS_BASE:
        return base_url.rstrip("/")
    with urllib.request.urlopen(
        "https://api.github.com/repos/alex09x/prod-code/commits/main", timeout=15
    ) as response:
        sha = json.load(response).get("sha")
    if not isinstance(sha, str) or re.fullmatch(r"[0-9a-fA-F]{40}", sha) is None:
        raise ValueError(
            "GitHub returned an invalid commit identifier for the guard bundle"
        )
    return f"https://raw.githubusercontent.com/alex09x/prod-code/{sha}/scripts/hooks"


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


def _download_component(url: str, dest_file: Path) -> None:
    with urllib.request.urlopen(url, timeout=15) as resp:
        body = resp.read()
    if not body:
        raise ValueError(f"Empty download from {url}")
    dest_file.write_bytes(body)


def _validate_bundle_dir(bundle_dir: Path, entry_name: str) -> None:
    entry = bundle_dir / entry_name
    if not _is_readable_nonempty_file(entry) or not os.access(entry, os.X_OK):
        raise ValueError(f"Staged entrypoint invalid or non-executable: {entry}")
    for rel in REQUIRED_GUARD_RELPATHS:
        if not _is_readable_nonempty_file(bundle_dir / rel):
            raise ValueError(
                f"Staged guard component invalid or empty: {bundle_dir / rel}"
            )


def _unlink_quietly(path: Path) -> None:
    try:
        path.unlink(missing_ok=True)
    except OSError:
        pass


def _rmdir_if_empty(path: Path) -> None:
    try:
        if path.is_dir() and not any(path.iterdir()):
            path.rmdir()
    except OSError:
        pass


def _write_launcher_tempfile(tmp_launcher: Path) -> None:
    fd = os.open(str(tmp_launcher), os.O_WRONLY | os.O_CREAT | os.O_EXCL, EXEC_MODE)
    try:
        os.write(fd, LAUNCHER_BYTES)
    finally:
        os.close(fd)
    os.chmod(tmp_launcher, EXEC_MODE)


def _restore_current_link(
    current_link: Path, prev_target: str | None, suffix: str
) -> None:
    if prev_target is None:
        _unlink_quietly(current_link)
        return
    restore_tmp = current_link.parent / f".restore.tmp.{suffix}"
    try:
        os.symlink(prev_target, restore_tmp)
        os.replace(restore_tmp, current_link)
    except OSError:
        _unlink_quietly(restore_tmp)


def _stage_bundle(
    src: Path, dest_name: str, versions_root: Path, base_url: str, suffix: str
) -> Path:
    src_dir = src.parent
    stage_dir = versions_root / f".staging.{suffix}"
    release_dir = versions_root / f"v.{time.time_ns():020d}_{suffix}"
    manifest = [(src, CANONICAL_HOOK_NAME, stage_dir / dest_name, True)]
    manifest.extend(
        (src_dir / rel, rel, stage_dir / rel, False) for rel in REQUIRED_GUARD_RELPATHS
    )
    use_local = src.is_file()

    try:
        remote_base = base_url if use_local else _immutable_remote_base(base_url)
        stage_dir.mkdir(parents=True, exist_ok=False)
        os.chmod(stage_dir, DIR_MODE)
        for local_src, remote_rel, staged_path, is_exec in manifest:
            if staged_path.parent != stage_dir:
                staged_path.parent.mkdir(parents=True, exist_ok=True)
                os.chmod(staged_path.parent, DIR_MODE)
            if use_local:
                if not local_src.is_file():
                    raise FileNotFoundError(
                        f"Required local hook component missing at {local_src}"
                    )
                shutil.copy2(local_src, staged_path)
            else:
                url = f"{remote_base.rstrip('/')}/{remote_rel}"
                try:
                    _download_component(url, staged_path)
                except Exception as exc:
                    raise FileNotFoundError(
                        f"Required hook component missing at {local_src} "
                        f"and failed to download from {url}: {exc}"
                    ) from exc
            os.chmod(staged_path, EXEC_MODE if is_exec else FILE_MODE)

        _validate_bundle_dir(stage_dir, dest_name)
        os.replace(stage_dir, release_dir)
        return release_dir
    except Exception:
        shutil.rmtree(stage_dir, ignore_errors=True)
        shutil.rmtree(release_dir, ignore_errors=True)
        raise


def deploy_canonical_script(
    src_path: Path, dest_path: str, base_url: str = RAW_GITHUB_HOOKS_BASE
) -> None:
    """Stages a complete guard version directory and activates it via one pointer swap."""
    src, dest = Path(src_path), Path(dest_path)
    dest.parent.mkdir(parents=True, exist_ok=True)
    lock_path = dest.parent / f".{dest.name}.install.lock"
    with lock_path.open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            _deploy_locked(src, dest, base_url)
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def _deploy_locked(src: Path, dest: Path, base_url: str) -> None:
    versions_root = versions_store_dir(dest)
    versions_root_existed = versions_root.exists()
    versions_root.mkdir(parents=True, exist_ok=True)

    current_link = versions_root / "current"
    prev_target = os.readlink(current_link) if current_link.is_symlink() else None
    launcher_ready = _is_valid_launcher(dest)
    suffix = f"{os.getpid()}_{os.urandom(4).hex()}"

    try:
        release_dir = _stage_bundle(src, dest.name, versions_root, base_url, suffix)
    except Exception:
        if not versions_root_existed:
            _rmdir_if_empty(versions_root)
        raise

    tmp_current = versions_root / f".current.tmp.{suffix}"
    if launcher_ready:
        try:
            os.symlink(release_dir.name, tmp_current)
            os.replace(tmp_current, current_link)
        except Exception:
            _unlink_quietly(tmp_current)
            shutil.rmtree(release_dir, ignore_errors=True)
            raise
    else:
        tmp_launcher = dest.with_name(f".{dest.name}.tmp.{suffix}")
        current_updated = False
        try:
            _write_launcher_tempfile(tmp_launcher)
            os.symlink(release_dir.name, tmp_current)
            os.replace(tmp_current, current_link)
            current_updated = True
            os.replace(tmp_launcher, dest)
        except Exception:
            _unlink_quietly(tmp_current)
            _unlink_quietly(tmp_launcher)
            if current_updated:
                _restore_current_link(current_link, prev_target, suffix)
            shutil.rmtree(release_dir, ignore_errors=True)
            if not versions_root_existed:
                _rmdir_if_empty(versions_root)
            raise


def check_canonical_hook(hook_path: str) -> tuple[bool, str]:
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
            for rel in REQUIRED_GUARD_RELPATHS
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
        for rel in REQUIRED_GUARD_RELPATHS
        if not _is_readable_nonempty_file(active_dir / rel)
    ]
    if missing:
        return (
            False,
            f"MISSING REQUIRED COMPONENTS ({', '.join(missing)}) in {active_dir}",
        )
    return (
        True,
        f"OK ({hook_path} -> {active_dir.name}, +{len(REQUIRED_GUARD_RELPATHS)} modules)",
    )
