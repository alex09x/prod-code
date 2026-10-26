#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""One final gate: the unchanged preservation target, then the native consumer.

Run at the repository root through prod-code exec on a Linux build node. No
daemon, installed binary, analyzer subprocess or external route supplies answers.
The native target retains its real requests, replies, sources and runtime output.
Normal runs write into a temporary directory unless --output-dir or
PROD_CODE_EVIDENCE_OUT explicitly names a caller-owned directory outside the
tracked historical root.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


EVIDENCE = Path("evidence/issue-1357")
COMMANDS = [
    ["cargo", "test", "-p", "prod-code-mcp", "--test", "rust_method_preservation"],
    ["cargo", "test", "-p", "prod-code-gateway", "--test", "rust_method_native"],
]


def resolve_output_dir(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output-dir",
        default=os.environ.get("PROD_CODE_EVIDENCE_OUT", "").strip(),
        help="Explicit caller-owned directory for gate logs, native artifacts, and hashes.",
    )
    args = parser.parse_args(argv)
    if args.output_dir:
        out_dir = Path(args.output_dir)
        out_dir.mkdir(parents=True, exist_ok=True)
        resolved = out_dir.resolve()
        if resolved in {EVIDENCE.resolve(), (EVIDENCE / "native-consumer").resolve()}:
            raise SystemExit(
                f"refusing to overwrite tracked historical evidence directory: {out_dir}"
            )
        return out_dir
    return Path(tempfile.mkdtemp(prefix="prod-code-issue-1357-"))


def record_hashes(out_dir: Path):
    cwd = Path.cwd().resolve()
    paths = set()
    for root in [
        "crates/prod-code-engine-rust/src/editor",
        "crates/prod-code-gateway/src/lsp_handlers",
        "crates/prod-code-gateway/src/session",
        "crates/prod-code-gateway/tests/rust_method_native",
        "crates/prod-code-mcp/src/to_method",
        "crates/prod-code-mcp/src/move_method",
        "crates/prod-code-mcp/tests/rust_method_preservation",
        str(EVIDENCE / "native-consumer"),
    ]:
        paths.update(Path(root).glob("**/*.rs"))
    native_out = out_dir / "native-consumer"
    if native_out.exists():
        for rs_file in native_out.glob("**/*.rs"):
            try:
                paths.add(rs_file.resolve().relative_to(cwd))
            except ValueError:
                pass
    paths.update([
        Path("crates/prod-code-gateway/tests/rust_method_native.rs"),
        Path("crates/prod-code-mcp/tests/rust_method_preservation.rs"),
        EVIDENCE / "run-final.py",
    ])
    preserved = json.loads(Path(
        "evidence/issue-1316/native-consumer/preserved-input-hashes.json"
    ).read_text())["files"]
    preserved_paths = {Path(item["path"]) for item in preserved}
    paths.update(preserved_paths)
    records = []
    for path in sorted(paths):
        records.append({
            "path": path.as_posix(),
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        })
    (out_dir / "source-hashes.json").write_text(json.dumps(records, indent=2) + "\n")
    original = {item["path"]: item["sha256"] for item in preserved}
    checks = [
        {**item, "matches_retained_input": item["sha256"] == original[item["path"]]}
        for item in records
        if Path(item["path"]) in preserved_paths
    ]
    (out_dir / "preserved-input-hashes.json").write_text(json.dumps(checks, indent=2) + "\n")

    artifact_records = []
    for artifact in sorted(p for p in out_dir.rglob("*") if p.is_file() and p.name != "artifact-hashes.json"):
        try:
            rel = artifact.resolve().relative_to(cwd).as_posix()
        except ValueError:
            rel = artifact.relative_to(out_dir).as_posix()
        artifact_records.append({
            "path": rel,
            "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        })
    (out_dir / "artifact-hashes.json").write_text(json.dumps(artifact_records, indent=2) + "\n")


def main(argv=None):
    out_dir = resolve_output_dir(sys.argv[1:] if argv is None else argv)
    native_dir = (out_dir / "native-consumer").resolve()
    native_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["PROD_CODE_NATIVE_ARTIFACT_DIR"] = str(native_dir)
    results = []
    exit_code = 0
    for command in COMMANDS:
        name = command[-1]
        print("$ " + " ".join(command), flush=True)
        completed = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=env,
        )
        output = completed.stdout.decode("utf-8", errors="replace")
        # Normalize only the remote checkout prefix; retain all diagnostics and exits.
        output = output.replace(str(Path.cwd()), "<checkout>")
        log = "$ " + " ".join(command) + "\n" + output + f"\nexit={completed.returncode}\n"
        (out_dir / f"{name}.txt").write_text(log)
        print(output, end="", flush=True)
        print(f"exit={completed.returncode}", flush=True)
        results.append({"command": command, "exit": completed.returncode})
        if completed.returncode:
            exit_code = completed.returncode
            break
    (out_dir / "gate-results.json").write_text(json.dumps(results, indent=2) + "\n")
    record_hashes(out_dir)
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
