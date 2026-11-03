#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Collect only expected native assets; verify transport before optional publication."""

import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

TRIPLETS = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
]
EXPECTED = {
    f"{binary}-{triplet}"
    for triplet in TRIPLETS
    for binary in ["prod-code", "prod-code-server"]
}


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def collect(incoming, output):
    output.mkdir()
    receipt = None
    seen = set()
    for archive_path in sorted(incoming.glob("*/*.tar")):
        with tarfile.open(archive_path) as archive:
            for member in archive.getmembers():
                if member.isdir() and member.name in [".", "./"]:
                    continue
                name = member.name.removeprefix("./")
                if (
                    not member.isfile()
                    or name not in EXPECTED | {"notarization.json"}
                    or name in seen
                    or member.size > 256 * 1024 * 1024
                ):
                    raise ValueError(
                        "unexpected, duplicate or unsafe release archive member"
                    )
                seen.add(name)
                with (
                    archive.extractfile(member) as source,
                    (output / name).open("wb") as dest,
                ):
                    shutil.copyfileobj(source, dest)
                if name == "notarization.json":
                    receipt = json.loads((output / name).read_text())
    if seen != EXPECTED | {"notarization.json"}:
        raise ValueError("missing native release assets or notarization receipt")
    if receipt.get("status") != "Accepted":
        raise ValueError("notarization was not accepted")
    mac = {n for n in EXPECTED if n.endswith("apple-darwin")}
    if set(receipt.get("sha256", {})) != mac:
        raise ValueError("notarization receipt does not cover exactly both Mac assets")
    for name in mac:
        if digest(output / name) != receipt["sha256"][name]:
            raise ValueError("signed Mac artifact differs from notarized bytes")
    (output / "notarization.json").unlink()
    manifest = "".join(
        f"{digest(output / name)}  {name}\n" for name in sorted(EXPECTED)
    )
    (output / "SHA256SUMS").write_text(manifest)
    return manifest


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True).strip()


def publish(tag, output):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ValueError("invalid release tag")
    existing = json.loads(
        gh("release", "list", "--limit", "100", "--json", "tagName,isDraft")
    )
    if any(r["tagName"] == tag for r in existing):
        raise ValueError(
            "release already exists; published bytes are never overwritten"
        )
    version = tuple(map(int, tag[1:].split(".")))
    stable = [
        r
        for r in existing
        if not r["isDraft"] and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", r["tagName"])
    ]
    if any(tuple(map(int, r["tagName"][1:].split("."))) >= version for r in stable):
        raise ValueError(
            "release version must be newer than the published stable versions"
        )
    gh(
        "release",
        "create",
        tag,
        "--verify-tag",
        "--draft",
        "--title",
        tag,
        "--notes",
        "Native client and gateway builds. macOS binaries are Developer ID signed "
        "and Apple notarized. Verify the six binaries using SHA256SUMS.",
        *[str(output / name) for name in sorted(EXPECTED | {"SHA256SUMS"})],
    )
    with tempfile.TemporaryDirectory() as directory:
        gh("release", "download", tag, "--dir", directory)
        downloaded = Path(directory)
        if {p.name for p in downloaded.iterdir()} != EXPECTED | {"SHA256SUMS"}:
            raise ValueError("downloaded release asset set differs")
        for name in EXPECTED | {"SHA256SUMS"}:
            if digest(downloaded / name) != digest(output / name):
                raise ValueError(
                    "downloaded draft asset digest differs; draft retained"
                )
    gh("release", "edit", tag, "--draft=false", "--latest")
    retained = sorted(
        [tag] + [r["tagName"] for r in stable],
        key=lambda item: tuple(map(int, item[1:].split("."))),
        reverse=True,
    )
    for old in retained[9:]:
        before = gh("api", f"repos/{os.environ['GH_REPO']}/git/ref/tags/{old}")
        gh("release", "delete", old, "--yes")
        after = gh("api", f"repos/{os.environ['GH_REPO']}/git/ref/tags/{old}")
        if json.loads(before)["object"] != json.loads(after)["object"]:
            raise ValueError("release retention unexpectedly altered a Git tag")


if __name__ == "__main__":
    root = Path(os.environ["RUNNER_TEMP"])
    out = root / "release"
    manifest = collect(root / "incoming", out)
    print(manifest)
    if os.environ.get("PUBLISH_RELEASE") == "true":
        publish(os.environ["RELEASE_TAG"], out)
    else:
        print("Dry run: no GitHub release created or changed")
