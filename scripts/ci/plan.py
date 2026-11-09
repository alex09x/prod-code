#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Plan bounded hosted test shards and validate immutable release inputs."""

import argparse
import json
import os
import re
import subprocess
from pathlib import Path

from scope import (
    SHA_RE,
    affected_workspace_packages,
    is_doc_file,
    is_version_bump_only,
)

SHARD_SIZE = 16
SKIPPED_MATRIX = {"include": [{"package": "none", "kind": "none", "targets": []}]}
VERSION_SMOKE_MATRIX = {
    "include": [
        {
            "package": "prod-code-protocol",
            "kind": "test",
            "targets": ["build_fingerprint"],
        }
    ]
}


def command(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def test_matrix(metadata, packages=None, shard_size=SHARD_SIZE):
    rows = []
    members = set(metadata["workspace_members"])
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        if packages is not None and package["name"] not in packages:
            continue
        targets = sorted(package["targets"], key=lambda item: item["name"])
        for target in targets:
            kind = next((k for k in ["lib", "bin"] if k in target["kind"]), None)
            if kind:
                rows.append(
                    {
                        "package": package["name"],
                        "kind": kind,
                        "targets": [target["name"]],
                    }
                )
                if kind == "lib" and target.get("doctest", True):
                    rows.append(
                        {
                            "package": package["name"],
                            "kind": "doc",
                            "targets": [target["name"]],
                        }
                    )
        tests = [t["name"] for t in targets if "test" in t["kind"]]
        for offset in range(0, len(tests), shard_size):
            rows.append(
                {
                    "package": package["name"],
                    "kind": "test",
                    "targets": tests[offset : offset + shard_size],
                }
            )
    if not rows or len(rows) > 256:
        raise ValueError("test matrix is empty or exceeds GitHub's 256-job limit")
    return {"include": rows}


def plan_ci(metadata, root, event_name="", base_sha="", head_sha="HEAD"):
    members = set(metadata["workspace_members"])
    ws_names = {p["name"] for p in metadata["packages"] if p["id"] in members}

    def full():
        return {
            "scope": "full",
            "run_lint": "true",
            "run_warm": "true",
            "run_tests": "true",
            "run_macos": "true",
            "matrix": require_integration_targets(test_matrix(metadata)),
        }

    if (
        event_name not in {"pull_request", "push"}
        or not base_sha
        or set(base_sha) == {"0"}
        or not SHA_RE.fullmatch(base_sha)
    ):
        return full()

    try:
        subprocess.run(
            ["git", "cat-file", "-e", f"{base_sha}^{{commit}}"],
            cwd=root,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if event_name == "pull_request":
            diff_base = command("git", "merge-base", base_sha, head_sha, cwd=root)
        else:
            subprocess.run(
                ["git", "merge-base", "--is-ancestor", base_sha, head_sha],
                cwd=root,
                check=True,
            )
            diff_base = base_sha
        raw = command("git", "diff", "--name-only", f"{diff_base}..{head_sha}", cwd=root)
    except subprocess.CalledProcessError:
        return full()

    changed = {line for line in raw.splitlines() if line}
    if not changed or all(is_doc_file(path) for path in changed):
        return {
            "scope": "docs",
            "run_lint": "false",
            "run_warm": "false",
            "run_tests": "false",
            "run_macos": "false",
            "matrix": SKIPPED_MATRIX,
        }

    if is_version_bump_only(root, diff_base, head_sha, changed, ws_names):
        return {
            "scope": "version",
            "run_lint": "false",
            "run_warm": "false",
            "run_tests": "true",
            "run_macos": "false",
            "matrix": VERSION_SMOKE_MATRIX,
        }

    if event_name == "pull_request":
        affected = affected_workspace_packages(metadata, root, changed)
        if affected is not None and affected < ws_names:
            matrix = test_matrix(metadata, packages=affected)
            needs_warm = any(
                r["package"] != "prod-code-protocol" for r in matrix["include"]
            )
            needs_macos = bool(affected & {"prod-code-client", "prod-code-protocol"})
            return {
                "scope": "affected",
                "run_lint": "true",
                "run_warm": "true" if needs_warm else "false",
                "run_tests": "true",
                "run_macos": "true" if needs_macos else "false",
                "matrix": matrix,
            }

    return full()


def release_input(root, tag):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ValueError("release tag must be vMAJOR.MINOR.PATCH")
    commit = command("git", "rev-parse", f"refs/tags/{tag}^{{commit}}", cwd=root)
    subprocess.run(
        ["git", "merge-base", "--is-ancestor", commit, "refs/remotes/origin/main"],
        cwd=root,
        check=True,
    )
    manifest = command("git", "show", f"{commit}:Cargo.toml", cwd=root)
    section = manifest.split("[workspace.package]", 1)[1].split("\n[", 1)[0]
    version = re.search(r'^version\s*=\s*"([^"]+)"\s*$', section, re.MULTILINE)
    if version is None or version[1] != tag[1:]:
        raise ValueError("tag does not match the committed workspace version")
    return {"tag": tag, "commit": commit, "version": version[1]}


def emit(name, value):
    encoded = (
        json.dumps(value, separators=(",", ":"))
        if not isinstance(value, str)
        else value
    )
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as stream:
            stream.write(f"{name}={encoded}\n")
    else:
        print(f"{name}={encoded}")


def run_tests(root, row):
    base = ["mitts", "test", "--locked", "-p", row["package"]]
    failed = []
    for target in row["targets"]:
        selector = (
            ["--lib"]
            if row["kind"] == "lib"
            else (["--doc"] if row["kind"] == "doc" else [f"--{row['kind']}", target])
        )
        # The tests of a shard share one pool as wide as the runner. The live gateway fixtures
        # each start real language servers and run one at a time; Mitts does not hand arguments
        # after `--` to the test processes, so the width is set by its own flag.
        concurrency = (
            "--concurrency=1"
            if (row["package"], row["kind"], target)
            == ("prod-code-gateway", "test", "live")
            else "--concurrency"
        )
        command_line = base + [concurrency] + selector
        if subprocess.run(command_line, cwd=root, check=False).returncode:
            failed.append(target)
    return failed


def require_integration_targets(matrix):
    if not any(row["kind"] == "test" for row in matrix["include"]):
        raise SystemExit(
            "Mitts metadata omitted integration test targets; "
            "refusing an incomplete CI matrix"
        )
    return matrix


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["tests", "release", "run-tests"])
    parser.add_argument("--root", type=Path, default=Path.cwd())
    args = parser.parse_args()
    if args.mode == "tests":
        metadata = json.loads(
            command(
                "mitts",
                "metadata",
                "--locked",
                "--format-version",
                "1",
                cwd=args.root,
            )
        )
        planned = plan_ci(
            metadata,
            args.root,
            event_name=os.environ.get("CI_EVENT_NAME", ""),
            base_sha=os.environ.get("CI_BASE_SHA", ""),
        )
        for key, value in planned.items():
            emit(key, value)
    elif args.mode == "release":
        for name, value in release_input(args.root, os.environ["RELEASE_TAG"]).items():
            emit(name, value)
    else:
        row = json.loads(os.environ["TEST_SHARD"])
        failed = run_tests(args.root, row)
        if failed:
            print("Failed targets:", ", ".join(failed))
            raise SystemExit(1)


if __name__ == "__main__":
    main()
