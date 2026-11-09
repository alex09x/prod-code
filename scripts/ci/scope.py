#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Change-aware hosted CI scope selection and final aggregator verification."""

import re
import subprocess
from pathlib import Path

ROOT_DOC_FILES = frozenset(
    {
        "AGENTS.md",
        "CHANGELOG.md",
        "CLAUDE.md",
        "CONTRIBUTING.md",
        "GEMINI.md",
        "LICENSE",
        "LICENSE-APACHE",
        "LICENSE-MIT",
        "README.md",
    }
)

# Integration tests that build and spawn another workspace binary at runtime.
IMPLICIT_REVERSE_TEST_DEPS = {
    "prod-code-gateway": {"prod-code-mcp"},
    "prod-code-client": {"prod-code-gateway"},
}

SEMVER_LINE_RE = re.compile(r'^(version\s*=\s*")([0-9]+\.[0-9]+\.[0-9]+)(")\s*$')
PACKAGE_NAME_RE = re.compile(r'^name\s*=\s*"([^"]+)"\s*$', re.MULTILINE)
SHA_RE = re.compile(r"^[0-9a-fA-F]{7,40}$")


def is_doc_file(path: str) -> bool:
    return path in ROOT_DOC_FILES or path.startswith("docs/")


def _git(root: Path, *args: str) -> str:
    return subprocess.check_output(
        ["git", *args], cwd=root, text=True, stderr=subprocess.DEVNULL
    ).strip()


def parse_workspace_version_only(old_toml: str, new_toml: str) -> bool:
    if old_toml == new_toml:
        return False

    def normalize(text: str):
        lines = text.splitlines()
        in_ws_pkg = False
        replaced = 0
        out = []
        for line in lines:
            stripped = line.strip()
            if stripped.startswith("[") and stripped.endswith("]"):
                in_ws_pkg = stripped == "[workspace.package]"
                out.append(line)
                continue
            if in_ws_pkg and SEMVER_LINE_RE.match(line):
                out.append('version = "0.0.0"')
                replaced += 1
            else:
                out.append(line)
        return out if replaced == 1 else None

    norm_old = normalize(old_toml)
    norm_new = normalize(new_toml)
    return norm_old is not None and norm_old == norm_new


def parse_lockfile_workspace_versions_only(
    old_lock: str, new_lock: str, workspace_names: set[str]
) -> bool:
    if old_lock == new_lock:
        return False
    old_parts = old_lock.split("\n[[package]]\n")
    new_parts = new_lock.split("\n[[package]]\n")
    if len(old_parts) < 2 or len(old_parts) != len(new_parts):
        return False
    if old_parts[0] != new_parts[0]:
        return False
    changed_blocks = 0
    for old_block, new_block in zip(old_parts[1:], new_parts[1:]):
        if old_block == new_block:
            continue
        old_name = PACKAGE_NAME_RE.search(old_block)
        new_name = PACKAGE_NAME_RE.search(new_block)
        if not old_name or not new_name or old_name.group(1) != new_name.group(1):
            return False
        if old_name.group(1) not in workspace_names:
            return False
        old_lines = old_block.splitlines()
        new_lines = new_block.splitlines()
        if len(old_lines) != len(new_lines):
            return False
        version_diffs = 0
        for old_line, new_line in zip(old_lines, new_lines):
            if old_line == new_line:
                continue
            if SEMVER_LINE_RE.match(old_line) and SEMVER_LINE_RE.match(new_line):
                version_diffs += 1
            else:
                return False
        if version_diffs != 1:
            return False
        changed_blocks += 1
    return changed_blocks > 0


def is_version_bump_only(
    root: Path, base_ref: str, head_ref: str, changed: set[str], workspace_names: set[str]
) -> bool:
    manifest_files = changed & {"Cargo.toml", "Cargo.lock"}
    if not manifest_files:
        return False
    if not all(is_doc_file(p) for p in changed - {"Cargo.toml", "Cargo.lock"}):
        return False
    try:
        if "Cargo.toml" in manifest_files:
            old_toml = _git(root, "show", f"{base_ref}:Cargo.toml")
            new_toml = _git(root, "show", f"{head_ref}:Cargo.toml")
            if not parse_workspace_version_only(old_toml, new_toml):
                return False
        if "Cargo.lock" in manifest_files:
            old_lock = _git(root, "show", f"{base_ref}:Cargo.lock")
            new_lock = _git(root, "show", f"{head_ref}:Cargo.lock")
            if not parse_lockfile_workspace_versions_only(
                old_lock, new_lock, workspace_names
            ):
                return False
    except subprocess.CalledProcessError:
        return False
    return True


def affected_workspace_packages(
    metadata: dict, root: Path, changed: set[str]
) -> set[str] | None:
    members = set(metadata["workspace_members"])
    ws_pkgs = [p for p in metadata["packages"] if p["id"] in members]
    ws_names = {p["name"] for p in ws_pkgs}
    resolved_root = root.resolve()
    prefixes: list[tuple[str, str]] = []
    for pkg in ws_pkgs:
        rel_dir = (
            Path(pkg["manifest_path"]).resolve().parent.relative_to(resolved_root).as_posix()
        )
        prefixes.append((rel_dir + "/", pkg["name"]))
    prefixes.sort(key=lambda item: len(item[0]), reverse=True)

    direct: set[str] = set()
    for path in changed:
        if is_doc_file(path):
            continue
        owner = next((name for prefix, name in prefixes if path.startswith(prefix)), None)
        if owner is None:
            return None
        direct.add(owner)

    reverse: dict[str, set[str]] = {name: set() for name in ws_names}
    for pkg in ws_pkgs:
        for dep in pkg.get("dependencies", []):
            dep_name = dep["name"]
            if dep_name in reverse:
                reverse[dep_name].add(pkg["name"])

    compile_affected = set(direct)
    queue = list(direct)
    while queue:
        current = queue.pop()
        for dependent in reverse.get(current, ()):
            if dependent not in compile_affected:
                compile_affected.add(dependent)
                queue.append(dependent)

    affected = set(compile_affected)
    for pkg_name in compile_affected:
        affected.update(IMPLICIT_REVERSE_TEST_DEPS.get(pkg_name, set()) & ws_names)
    return affected


VALID_FLAGS = {"true", "false"}
VALID_SCOPES = {"docs", "version", "affected", "full"}


def verify_ci_results(needs: dict) -> None:
    if needs.get("plan", {}).get("result") != "success":
        raise AssertionError(f"plan did not succeed: {needs}")
    outputs = needs["plan"].get("outputs") or {}
    scope = outputs.get("scope")
    if scope not in VALID_SCOPES:
        raise AssertionError(f"invalid or missing plan scope: {outputs}")
    flags = {
        key: outputs.get(key)
        for key in ("run_lint", "run_warm", "run_tests", "run_macos")
    }
    if any(value not in VALID_FLAGS for value in flags.values()):
        raise AssertionError(f"missing or malformed plan flags: {outputs}")
    if scope == "docs" and any(v != "false" for v in flags.values()):
        raise AssertionError(f"docs scope must skip heavy jobs: {outputs}")
    if scope == "version" and flags != {
        "run_lint": "true",
        "run_warm": "false",
        "run_tests": "false",
        "run_macos": "true",
    }:
        raise AssertionError(f"unexpected version scope flags: {outputs}")
    if scope == "affected" and (
        flags["run_lint"] != "true"
        or flags["run_warm"] != "true"
        or flags["run_tests"] != "true"
    ):
        raise AssertionError(f"affected scope must run lint/warm/tests: {outputs}")
    if scope == "full" and any(v != "true" for v in flags.values()):
        raise AssertionError(f"full scope must run every job: {outputs}")

    expected = {
        "lint": "success" if flags["run_lint"] == "true" else "skipped",
        "warm": "success" if flags["run_warm"] == "true" else "skipped",
        "test": "success" if flags["run_tests"] == "true" else "skipped",
        "macos-signatures": (
            "success" if flags["run_macos"] == "true" else "skipped"
        ),
    }
    actual = {name: needs.get(name, {}).get("result") for name in expected}
    if actual != expected:
        raise AssertionError(f"CI scope mismatch: expected={expected}, actual={actual}")
