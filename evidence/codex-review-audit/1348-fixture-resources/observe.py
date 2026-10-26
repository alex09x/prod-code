# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Bounded Linux /proc sampling of this command's test child; no paths/addresses logged."""
import collections
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time


def children(pid):
    try:
        return Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
    except FileNotFoundError:
        return []


def snapshot(pid):
    root = Path(f"/proc/{pid}")
    counts = collections.Counter()
    socket_inodes = set()
    empty_watchers = 0
    watches = 0
    for fd in (root / "fd").iterdir():
        try:
            target = os.readlink(fd)
            if target.startswith("socket:"):
                counts["socket"] += 1
                socket_inodes.add(target[8:-1])
            elif target.startswith("pipe:"):
                counts["pipe"] += 1
            elif "inotify" in target:
                counts["inotify"] += 1
                info = (root / "fdinfo" / fd.name).read_text()
                n = sum(line.startswith("inotify wd:") for line in info.splitlines())
                watches += n
                empty_watchers += n == 0
            elif target.startswith("anon_inode:"):
                counts[target] += 1
            else:
                counts["file"] += 1
        except OSError:
            continue
    states = collections.Counter()
    for family in ["tcp", "tcp6"]:
        for line in (root / "net" / family).read_text().splitlines()[1:]:
            fields = line.split()
            if fields[9] in socket_inodes:
                states[fields[3]] += 1
    limits = next(line.split()[3:5] for line in (root / "limits").read_text().splitlines()
                  if line.startswith("Max open files"))
    return {"fds": sum(counts.values()), "kinds": dict(counts),
            "socket_states_hex": dict(states), "inotify_watches": watches,
            "empty_inotify": empty_watchers, "nofile": limits}


prefix, *command = sys.argv[1:]
started = time.monotonic()
samples = []
last = None
peak = None
with open(prefix + ".txt", "w+") as log:
    process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
    try:
        while process.poll() is None:
            for pid in children(process.pid):
                try:
                    name = Path(f"/proc/{pid}/comm").read_text().strip()
                    if not name.startswith("rust_method_pre"):
                        continue
                    current = snapshot(pid)
                    if peak is None or current["fds"] > peak["fds"]:
                        peak = current
                    if last is None or abs(current["fds"] - last["fds"]) >= 32:
                        samples.append({"seconds": round(time.monotonic() - started, 3), **current})
                        last = current
                except OSError:
                    # /proc entries can disappear or become unreadable during exit.
                    pass
            time.sleep(0.02)
    finally:
        # Even a failed observer must join the command it started.
        process.wait()
    log.seek(0)
    output = log.read().replace(str(Path.cwd()), "<remote-checkout>")
    output = re.sub(r"/tmp/\.tmp[A-Za-z0-9]+", "<fixture>", output)
    log.seek(0)
    log.write(output)
    log.truncate()
result = {"command": command, "exit": process.returncode, "peak": peak, "samples": samples}
Path(prefix + ".json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps({"exit": process.returncode, "peak": peak, "samples": len(samples)}))
print(output[-1600:])
sys.exit(process.returncode)
