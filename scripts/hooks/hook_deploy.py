# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Versioned staging and atomic activation for the canonical prod-code guard hook."""

import fcntl
import importlib.util
import json
import os
import re
import shutil
import time
import urllib.request
from pathlib import Path
from types import ModuleType

_HOOKS_DIR = Path(__file__).resolve().parent


def _load_sibling_module(module_name: str, filename: str) -> ModuleType:
    module_path = _HOOKS_DIR / filename
    spec = importlib.util.spec_from_file_location(module_name, str(module_path))
    if spec is None or spec.loader is None:
        raise ImportError(f"Unable to load installer module at {module_path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_launcher_mod = _load_sibling_module("_prod_code_hook_launcher", "hook_launcher.py")

CANONICAL_HOOK_NAME = "prod-code-local-build-guard.py"
RAW_GITHUB_HOOKS_BASE = (
    "https://raw.githubusercontent.com/alex09x/prod-code/main/scripts/hooks"
)

GUARD_PACKAGE_DIR = "prod_code_guard"
REQUIRED_GUARD_RELPATHS = (
    f"{GUARD_PACKAGE_DIR}/__init__.py",
    f"{GUARD_PACKAGE_DIR}/shell.py",
    f"{GUARD_PACKAGE_DIR}/grep_patterns.py",
    f"{GUARD_PACKAGE_DIR}/grep_guard.py",
    f"{GUARD_PACKAGE_DIR}/sed_guard.py",
    f"{GUARD_PACKAGE_DIR}/shell_lex.py",
    f"{GUARD_PACKAGE_DIR}/shell_python.py",
    f"{GUARD_PACKAGE_DIR}/python_write_eval.py",
    f"{GUARD_PACKAGE_DIR}/python_write_guard.py",
    f"{GUARD_PACKAGE_DIR}/payload.py",
    f"{GUARD_PACKAGE_DIR}/rename_guard.py",
    f"{GUARD_PACKAGE_DIR}/dispatch.py",
)
REQUIRED_GUARD_COMPONENTS = (CANONICAL_HOOK_NAME, *REQUIRED_GUARD_RELPATHS)

LAUNCHER_MARKER = _launcher_mod.LAUNCHER_MARKER
EXEC_MODE = _launcher_mod.EXEC_MODE
FILE_MODE = _launcher_mod.FILE_MODE
DIR_MODE = _launcher_mod.DIR_MODE
LAUNCHER_SCRIPT = _launcher_mod.LAUNCHER_SCRIPT
LAUNCHER_BYTES = _launcher_mod.LAUNCHER_BYTES
versions_store_dir = _launcher_mod.versions_store_dir
_has_valid_launcher_bytes = _launcher_mod._has_valid_launcher_bytes
_is_valid_launcher = _launcher_mod._is_valid_launcher
_is_readable_nonempty_file = _launcher_mod._is_readable_nonempty_file
resolve_active_version_dir = _launcher_mod.resolve_active_version_dir
resolve_active_entrypoint = _launcher_mod.resolve_active_entrypoint


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
    return _launcher_mod.check_canonical_hook_bundle(hook_path, REQUIRED_GUARD_RELPATHS)
