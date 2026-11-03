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


def command(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def test_matrix(metadata):
    rows = []
    for package in metadata["packages"]:
        if package["id"] not in metadata["workspace_members"]:
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
        for offset in range(0, len(tests), 8):
            rows.append(
                {
                    "package": package["name"],
                    "kind": "test",
                    "targets": tests[offset : offset + 8],
                }
            )
    if not rows or len(rows) > 256:
        raise ValueError("test matrix is empty or exceeds GitHub's 256-job limit")
    return {"include": rows}


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
    base = ["cargo", "test", "--locked", "-p", row["package"]]
    failed = []
    for target in row["targets"]:
        selector = (
            ["--lib"]
            if row["kind"] == "lib"
            else (["--doc"] if row["kind"] == "doc" else [f"--{row['kind']}", target])
        )
        runtime = (
            ["--", "--test-threads=1"]
            if (row["package"], row["kind"], target)
            == ("prod-code-gateway", "test", "live")
            else []
        )
        if subprocess.run(base + selector + runtime, cwd=root, check=False).returncode:
            failed.append(target)
    return failed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["tests", "release", "run-tests"])
    parser.add_argument("--root", type=Path, default=Path.cwd())
    args = parser.parse_args()
    if args.mode == "tests":
        metadata = json.loads(
            command(
                "cargo",
                "metadata",
                "--locked",
                "--no-deps",
                "--format-version",
                "1",
                cwd=args.root,
            )
        )
        emit("matrix", test_matrix(metadata))
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
