#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
Live monitor and telemetry analyzer for prod-code AI agent guard hooks.
Displays real-time hook interceptions, multi-file rename blocks, build denials,
and symbol lookup redirections across Claude Code, Codex, and Antigravity.

Usage:
    python3 scripts/guard-monitor.py             # Show summary stats and recent events
    python3 scripts/guard-monitor.py --follow    # Tail events in real-time (like tail -f)
    python3 scripts/guard-monitor.py --renames   # Inspect only multi-file rename events
"""

import argparse
import collections
import datetime
import json
import os
import sys
import time
from pathlib import Path

DEFAULT_LOG = os.path.expanduser("~/.claude/hooks/prod-code-guard.jsonl")

# ANSI color codes
RESET = "\033[0m"
BOLD = "\033[1m"
RED = "\033[31m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
BLUE = "\033[34m"
MAGENTA = "\033[35m"
CYAN = "\033[36m"
GRAY = "\033[90m"


def color_decision(dec: str) -> str:
    if dec == "deny":
        return f"{RED}{BOLD}DENY{RESET}"
    elif dec == "override":
        return f"{GREEN}{BOLD}OVERRIDE{RESET}"
    elif dec == "override-refused":
        return f"{YELLOW}{BOLD}REFUSED{RESET}"
    elif dec == "first-file-tracked":
        return f"{CYAN}{BOLD}TRACKED{RESET}"
    return f"{GRAY}{dec}{RESET}"


def color_rule(rule: str) -> str:
    if rule == "multi-file-rename":
        return f"{MAGENTA}{BOLD}{rule}{RESET}"
    elif rule == "local-build":
        return f"{YELLOW}{rule}{RESET}"
    elif rule == "symbol-grep":
        return f"{BLUE}{rule}{RESET}"
    return rule


def format_event(d: dict) -> str:
    ts = d.get("ts", 0)
    dt_str = datetime.datetime.fromtimestamp(ts).strftime("%H:%M:%S") if ts else "--:--:--"
    agent = d.get("agent", "unknown")
    rule = d.get("rule", "unknown")
    decision = d.get("decision", "unknown")
    sym = d.get("symbol", "")
    target = d.get("file") or d.get("cmd") or ""
    prev_file = d.get("prev_file", "")

    rule_str = color_rule(rule)
    dec_str = color_decision(decision)
    agent_str = f"{BOLD}{agent:<11}{RESET}"

    extra = ""
    if rule == "multi-file-rename":
        cur_base = os.path.basename(target) if target else ""
        if prev_file:
            prev_base = os.path.basename(prev_file)
            extra = f"symbol='{BOLD}{sym}{RESET}' in {cur_base} (prev: {prev_base})"
        else:
            extra = f"symbol='{BOLD}{sym}{RESET}' in {cur_base}"
    elif rule == "symbol-grep":
        extra = f"sym='{sym}' cmd='{target[:65]}'"
    elif rule == "local-build":
        extra = f"cmd='{target[:65]}'"

    return f"[{GRAY}{dt_str}{RESET}] {agent_str} {rule_str:<26} {dec_str:<20} {extra}"


def read_all_events(log_path: str):
    if not os.path.isfile(log_path):
        return []
    events = []
    with open(log_path, "r", encoding="utf-8", errors="ignore") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except Exception:
                pass
    return events


def show_summary(events: list[dict], log_path: str, last_n: int = 10, agent_filter: str | None = None):
    if agent_filter:
        events = [e for e in events if e.get("agent") == agent_filter]

    print(f"{BOLD}======================================================================{RESET}")
    print(f"{BOLD} prod-code AI Agent Guard Telemetry Summary{RESET}")
    print(f"{BOLD}======================================================================{RESET}")
    print(f"Log path : {log_path}")
    if agent_filter:
        print(f"Agent filter : {agent_filter}")
    print(f"Total entries : {len(events)}")

    if not events:
        print("\nNo events found in log.")
        return

    first_ts = events[0].get("ts", 0)
    last_ts = events[-1].get("ts", 0)
    first_dt = datetime.datetime.fromtimestamp(first_ts).strftime("%Y-%m-%d %H:%M:%S") if first_ts else "?"
    last_dt = datetime.datetime.fromtimestamp(last_ts).strftime("%Y-%m-%d %H:%M:%S") if last_ts else "?"
    print(f"Time span : {first_dt} -> {last_dt}")
    print("----------------------------------------------------------------------")

    # Counts
    by_agent = collections.Counter(e.get("agent", "unknown") for e in events)
    by_rule = collections.Counter(e.get("rule", "unknown") for e in events)
    by_dec = collections.Counter(e.get("decision", "unknown") for e in events)
    by_rule_dec = collections.Counter((e.get("rule", "unknown"), e.get("decision", "unknown")) for e in events)

    print(f"{BOLD}Activity by Agent Provider:{RESET}")
    for ag, cnt in by_agent.most_common():
        pct = (cnt / len(events)) * 100
        print(f"  {ag:<20} : {cnt:>5} ({pct:>5.1f}%)")

    print(f"\n{BOLD}Interceptions by Rule & Decision:{RESET}")
    for (r, d), cnt in by_rule_dec.most_common():
        print(f"  {r:<20} {color_decision(d):<22} : {cnt:>5}")

    # Renames specifically
    rename_events = [e for e in events if e.get("rule") == "multi-file-rename"]
    print(f"\n{BOLD}Multi-File Rename Guard Activity:{RESET}")
    print(f"  Total rename evaluations: {len(rename_events)}")
    if rename_events:
        denies = [e for e in rename_events if e.get("decision") == "deny"]
        overrides = [e for e in rename_events if e.get("decision") == "override"]
        tracked = [e for e in rename_events if e.get("decision") == "first-file-tracked"]
        print(f"  - Blocked attempts (forced code_rename) : {RED}{len(denies)}{RESET}")
        print(f"  - First-file candidates tracked         : {CYAN}{len(tracked)}{RESET}")
        print(f"  - Explicit user overrides               : {GREEN}{len(overrides)}{RESET}")

    # Recent N events
    n_display = min(last_n, len(events))
    print(f"\n{BOLD}Recent {n_display} Guard Events:{RESET}")
    for e in events[-n_display:]:
        print("  " + format_event(e))

    print(f"{BOLD}======================================================================{RESET}")


def follow_log(log_path: str, filter_renames: bool = False, agent_filter: str | None = None):
    print(f"{BOLD}Watching prod-code guard events in real-time from {log_path}...{RESET}")
    print("Press Ctrl+C to exit.\n")
    if not os.path.isfile(log_path):
        print(f"Waiting for log file {log_path} to be created...")

    while not os.path.isfile(log_path):
        time.sleep(0.5)

    with open(log_path, "r", encoding="utf-8", errors="ignore") as f:
        # Seek to end
        f.seek(0, os.SEEK_END)
        while True:
            line = f.readline()
            if not line:
                time.sleep(0.1)
                continue
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
                if filter_renames and entry.get("rule") != "multi-file-rename":
                    continue
                if agent_filter and entry.get("agent") != agent_filter:
                    continue
                print(format_event(entry))
                sys.stdout.flush()
            except Exception:
                pass


def main():
    parser = argparse.ArgumentParser(description="Live monitor and analyzer for prod-code AI agent guard hooks.")
    parser.add_argument("--log", type=str, default=DEFAULT_LOG, help=f"Path to guard log file (default: {DEFAULT_LOG})")
    parser.add_argument("--follow", "-f", action="store_true", help="Continuously tail and print new events in real-time")
    parser.add_argument("--renames", "-r", action="store_true", help="Filter and display only multi-file rename events")
    parser.add_argument("--lines", "-n", "--last", dest="lines", type=int, default=10, help="Number of recent events to display in summary (default: 10)")
    parser.add_argument("--agent", "-a", type=str, default=None, help="Filter events by agent provider (e.g. antigravity, Bash, Edit, apply_patch)")
    args = parser.parse_args()

    log_path = os.path.expanduser(args.log)

    if args.follow:
        try:
            follow_log(log_path, filter_renames=args.renames, agent_filter=args.agent)
        except KeyboardInterrupt:
            print("\nStopped monitoring.")
            return 0
    else:
        events = read_all_events(log_path)
        if args.renames:
            events = [e for e in events if e.get("rule") == "multi-file-rename"]
        show_summary(events, log_path, last_n=args.lines, agent_filter=args.agent)
        return 0


if __name__ == "__main__":
    sys.exit(main())
