#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Verify installer trust boundaries using a public signed release, without keys."""

import hashlib
import json
import os
import platform
import re
import subprocess
import tempfile
import urllib.request
from pathlib import Path

REPOSITORY = "alex09x/prod-code"
ASSET = "prod-code-aarch64-apple-darwin"
REQUIREMENT = (
    '=anchor apple generic and certificate leaf[subject.OU] = "284V2M3LN9" '
    "and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
)
REQUIRED_TESTS = {
    "update::behavior_tests::": (
        "update::behavior_tests::"
        "unsigned_corrupted_or_adhoc_macos_update_is_rejected_and_preserves_the_executable"
    ),
    "package::ops::": (
        "package::ops::tests::"
        "download_and_install_rejects_unsigned_tampered_or_adhoc_macos_binary_and_preserves_existing"
    ),
}


def download(tag, name, destination):
    url = f"https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"
    with urllib.request.urlopen(url, timeout=120) as response:
        destination.write_bytes(response.read())


def main():
    if (
        os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
        or platform.system() != "Darwin"
        or platform.machine() != "arm64"
    ):
        raise SystemExit(
            "This fixture is restricted to disposable hosted Apple Silicon runners"
        )
    destination = Path.home() / ".local/bin/prod-code"
    if destination.exists() or destination.is_symlink():
        raise SystemExit("Refusing to replace an existing fixture executable")
    release = json.loads(
        subprocess.check_output(
            ["gh", "api", f"repos/{REPOSITORY}/releases/latest"], text=True
        )
    )
    tag = release["tag_name"]
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise SystemExit("The public fixture must come from a stable versioned release")
    with tempfile.TemporaryDirectory(prefix="prod-code-signed-fixture-") as temporary:
        directory = Path(temporary)
        manifest = directory / "SHA256SUMS"
        binary = directory / ASSET
        download(tag, "SHA256SUMS", manifest)
        download(tag, ASSET, binary)
        hashes = [
            match.group(1)
            for line in manifest.read_text().splitlines()
            if (match := re.fullmatch(r"([0-9a-f]{64})\s+\*?" + ASSET, line))
        ]
        actual = hashlib.sha256(binary.read_bytes()).hexdigest()
        if hashes != [actual]:
            raise SystemExit("Public fixture SHA256SUMS verification failed")
        binary.chmod(0o755)
        subprocess.run(
            [
                "codesign",
                "--verify",
                "--deep",
                "--strict",
                "-R",
                REQUIREMENT,
                str(binary),
            ],
            check=True,
        )
        destination.parent.mkdir(parents=True, exist_ok=True)
        # Atomic creation cannot clobber a destination created after the guard above.
        with destination.open("xb") as output:
            output.write(binary.read_bytes())
        destination.chmod(0o755)
        print(f"Public signed fixture: {tag}, SHA-256 {actual}", flush=True)
    try:
        for prefix, required in REQUIRED_TESTS.items():
            result = subprocess.run(
                [
                    "cargo",
                    "test",
                    "--locked",
                    "-p",
                    "prod-code-client",
                    "--bin",
                    "prod-code",
                    "--",
                    prefix,
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                check=False,
            )
            print(result.stdout, end="", flush=True)
            if result.returncode:
                raise SystemExit(result.returncode)
            if f"test {required} ... ok" not in result.stdout:
                raise SystemExit(
                    f"The required native trust regression did not execute: {required}"
                )
    finally:
        destination.unlink()


if __name__ == "__main__":
    main()
