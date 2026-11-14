#!/usr/bin/python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Run an editor bridge with private, rotating diagnostics; never inspect its stdio."""

import argparse
import json
import logging
import os
import select
import shutil
import signal
import subprocess
import sys
import threading
import time
from logging.handlers import RotatingFileHandler
from pathlib import Path


def logger(path, maximum):
    result = logging.getLogger(str(path))
    result.setLevel(logging.INFO)
    result.propagate = False
    handler = RotatingFileHandler(
        path, maxBytes=maximum, backupCount=3, encoding="utf-8"
    )
    handler.setFormatter(logging.Formatter("%(asctime)s %(message)s"))
    result.addHandler(handler)
    return result


def retain_sessions(root):
    """Prune only our completed sessions, retaining twenty plus every live session.

    Several language servers start at once and prune the same directory: a session another
    launcher removed meanwhile is skipped, never an error that stops this bridge from starting.
    """
    sessions = []
    for directory in root.glob("session-*"):
        try:
            meta = json.loads((directory / "session.json").read_text())
            if meta.get("logger") != "prod-code-zed":
                continue
            if "exit_code" not in meta:
                try:
                    os.kill(meta["wrapper_pid"], 0)
                    continue
                except ProcessLookupError:
                    pass
            sessions.append((directory.stat().st_mtime, directory))
        except (OSError, ValueError, KeyError):
            continue
    sessions.sort(key=lambda session: session[0], reverse=True)
    for _, directory in sessions[20:]:
        shutil.rmtree(directory, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--language",
        required=True,
        choices=["rust", "go", "cpp", "python", "typescript", "swift"],
    )
    parser.add_argument("--binary", default=str(Path.home() / ".local/bin/prod-code"))
    parser.add_argument(
        "--log-dir", type=Path, default=Path.home() / "Library/Logs/prod-code/zed"
    )
    parser.add_argument("--max-log-bytes", type=int, default=2 * 1024 * 1024)
    args = parser.parse_args()
    if args.max_log_bytes < 1024:
        parser.error("--max-log-bytes must be at least 1024")
    os.umask(0o077)
    args.log_dir.mkdir(parents=True, exist_ok=True)
    retain_sessions(args.log_dir)
    directory = args.log_dir / (
        f"session-{int(time.time())}-{args.language}-{os.getpid()}"
    )
    directory.mkdir(mode=0o700)
    trace = logger(directory / "transport.log", args.max_log_bytes)
    errors = logger(directory / "stderr.log", args.max_log_bytes)
    metadata = {
        "logger": "prod-code-zed",
        "wrapper_pid": os.getpid(),
        "language": args.language,
        "cwd": os.getcwd(),
        "binary": args.binary,
        "started_at": time.time(),
    }
    try:
        metadata["version"] = subprocess.check_output(
            [args.binary, "--version"], stderr=subprocess.STDOUT, timeout=5, text=True
        ).strip()
        # A bare name is found on PATH, as the two launches of it are.
        metadata["binary_inode"] = os.stat(
            shutil.which(args.binary) or args.binary
        ).st_ino
    except (OSError, subprocess.SubprocessError) as error:
        metadata["version_probe_error"] = str(error)
    fifo = directory / "trace.pipe"
    os.mkfifo(fifo, 0o600)
    descriptor = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
    stopped = threading.Event()

    def collect_trace():
        pending = b""
        while True:
            if not select.select([descriptor], [], [], 0.2)[0]:
                if stopped.is_set():
                    break
                continue
            pending += os.read(descriptor, 4096)
            while b"\n" in pending or len(pending) >= 4096:
                cut = pending.find(b"\n", 0, 4096)
                cut = cut + 1 if cut >= 0 else min(len(pending), 4096)
                trace.info(
                    "%s", pending[:cut].decode("utf-8", errors="replace").rstrip()
                )
                pending = pending[cut:]
        if pending:
            trace.info("%s", pending.decode("utf-8", errors="replace"))

    env = os.environ.copy()
    env["PROD_CODE_LSP_TRACE"] = str(fifo)
    child = None
    threads = []
    try:
        child = subprocess.Popen(
            [args.binary, "lsp", "--language", args.language],
            env=env,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        metadata["bridge_pid"] = child.pid
        (directory / "session.json").write_text(json.dumps(metadata, indent=2))

        def forward_signal(number, _frame):
            try:
                os.killpg(child.pid, number)
            except ProcessLookupError:
                pass

        signal.signal(signal.SIGTERM, forward_signal)
        signal.signal(signal.SIGINT, forward_signal)

        def collect_stderr():
            while True:
                chunk = child.stderr.read1(4096)
                if not chunk:
                    break
                errors.info("%s", chunk.decode("utf-8", errors="replace").rstrip())
                try:
                    sys.stderr.buffer.write(chunk)
                    sys.stderr.buffer.flush()
                except OSError:
                    pass

        threads = [
            threading.Thread(target=collect_trace),
            threading.Thread(target=collect_stderr),
        ]
        for thread in threads:
            thread.start()
        trace.info(
            "bridge_started pid=%s version=%s", child.pid, metadata.get("version")
        )
        code = child.wait()
        trace.info("bridge_exited code=%s", code)
        metadata["exit_code"] = code
        return code if code >= 0 else 128 - code
    finally:
        stopped.set()
        for thread in threads:
            thread.join(timeout=3)
        os.close(descriptor)
        fifo.unlink(missing_ok=True)
        metadata["finished_at"] = time.time()
        (directory / "session.json").write_text(json.dumps(metadata, indent=2))
        logging.shutdown()


if __name__ == "__main__":
    sys.exit(main())
