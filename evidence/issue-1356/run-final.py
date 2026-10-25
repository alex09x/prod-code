#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""One logical lifecycle gate: default parallelism and inherited resource limits."""

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[2]
SOURCES = [
    "crates/prod-code-mcp/src/session/mod.rs",
    "crates/prod-code-mcp/src/session/lifecycle.rs",
    "crates/prod-code-mcp/src/session/pool.rs",
    "crates/prod-code-mcp/src/session/tests/mod.rs",
    "crates/prod-code-mcp/src/session/tests/lifecycle.rs",
    "crates/prod-code-mcp/src/watch.rs",
    "crates/prod-code-mcp/src/watch/lifecycle.rs",
    "crates/prod-code-mcp/src/watch/tests/lifecycle.rs",
    "evidence/issue-1356/run-final.py",
]


def public_output(text):
    text = text.replace(str(ROOT), "<checkout>")
    text = re.sub(r"/(?:home|Users)/[^\s)]+", "<build-path>", text)
    return text


def main():
    if Path.cwd().resolve() != ROOT:
        raise SystemExit("Run from the assigned checkout root")
    hashes = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES}
    print("Source hashes: " + json.dumps(hashes, sort_keys=True), flush=True)
    try:
        import resource
        print(f"Inherited RLIMIT_NOFILE={resource.getrlimit(resource.RLIMIT_NOFILE)}", flush=True)
    except ImportError:
        print("RLIMIT_NOFILE unavailable on this platform", flush=True)
    env = os.environ.copy()
    # Show bounded FD evidence without serializing the harness or changing cargo argv.
    env["RUST_TEST_NOCAPTURE"] = "1"
    print("No explicit cleanup/disconnect, limit changes, or test serialization", flush=True)
    for module in ["session::tests", "watch::tests"]:
        command = ["cargo", "test", "-p", "prod-code-mcp", "--lib", module]
        print("$ " + " ".join(command), flush=True)
        result = subprocess.run(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        output = public_output(result.stdout)
        print(output, end="", flush=True)
        print(f"exit_code={result.returncode}", flush=True)
        if result.returncode:
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
