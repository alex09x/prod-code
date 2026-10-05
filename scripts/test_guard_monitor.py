#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Unit tests for prod-code guard monitor (scripts/guard-monitor.py)."""

import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

MONITOR_PATH = Path(__file__).resolve().parent / "guard-monitor.py"
spec = importlib.util.spec_from_file_location("guard_monitor", str(MONITOR_PATH))
monitor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(monitor)


class GuardMonitorTests(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmpdir.cleanup)
        self.log_path = Path(self.tmpdir.name) / "test-guard.jsonl"

    def test_read_all_events(self):
        entries = [
            {"ts": 1700000001, "agent": "antigravity", "rule": "symbol-grep", "decision": "deny", "symbol": "foo", "cmd": "rg foo"},
            {"ts": 1700000002, "agent": "Edit", "rule": "multi-file-rename", "decision": "first-file-tracked", "symbol": "bar", "file": "/path/a.py"},
            {"ts": 1700000003, "agent": "Edit", "rule": "multi-file-rename", "decision": "deny", "symbol": "bar", "file": "/path/b.py", "prev_file": "/path/a.py"},
        ]
        with open(self.log_path, "w", encoding="utf-8") as f:
            for entry in entries:
                f.write(json.dumps(entry) + "\n")

        events = monitor.read_all_events(str(self.log_path))
        self.assertEqual(len(events), 3)
        self.assertEqual(events[0]["symbol"], "foo")
        self.assertEqual(events[2]["decision"], "deny")

    def test_format_event(self):
        e1 = {"ts": 1700000001, "agent": "antigravity", "rule": "symbol-grep", "decision": "deny", "symbol": "foo", "cmd": "rg foo"}
        formatted = monitor.format_event(e1)
        self.assertIn("antigravity", formatted)
        self.assertIn("symbol-grep", formatted)
        self.assertIn("DENY", formatted)
        self.assertIn("foo", formatted)

        e2 = {"ts": 1700000003, "agent": "Edit", "rule": "multi-file-rename", "decision": "deny", "symbol": "bar", "file": "/path/b.py", "prev_file": "/path/a.py"}
        formatted2 = monitor.format_event(e2)
        self.assertIn("multi-file-rename", formatted2)
        self.assertIn("b.py", formatted2)
        self.assertIn("a.py", formatted2)

    def test_show_summary_output(self):
        events = [
            {"ts": 1700000001, "agent": "antigravity", "rule": "symbol-grep", "decision": "deny", "symbol": "foo", "cmd": "rg foo"},
            {"ts": 1700000002, "agent": "Edit", "rule": "multi-file-rename", "decision": "first-file-tracked", "symbol": "bar", "file": "/path/a.py"},
            {"ts": 1700000003, "agent": "Edit", "rule": "multi-file-rename", "decision": "deny", "symbol": "bar", "file": "/path/b.py", "prev_file": "/path/a.py"},
            {"ts": 1700000004, "agent": "Bash", "rule": "local-build", "decision": "deny", "cmd": "cargo build"},
        ]
        out = io.StringIO()
        old_stdout = sys.stdout
        try:
            sys.stdout = out
            monitor.show_summary(events, str(self.log_path), last_n=2)
        finally:
            sys.stdout = old_stdout

        output = out.getvalue()
        self.assertIn("Total entries : 4", output)
        self.assertIn("antigravity", output)
        self.assertIn("Recent 2 Guard Events", output)
        self.assertIn("Multi-File Rename Guard Activity", output)

    def test_show_summary_with_agent_filter(self):
        events = [
            {"ts": 1700000001, "agent": "antigravity", "rule": "symbol-grep", "decision": "deny", "symbol": "foo"},
            {"ts": 1700000002, "agent": "Edit", "rule": "multi-file-rename", "decision": "deny", "symbol": "bar"},
        ]
        out = io.StringIO()
        old_stdout = sys.stdout
        try:
            sys.stdout = out
            monitor.show_summary(events, str(self.log_path), agent_filter="Edit")
        finally:
            sys.stdout = old_stdout

        output = out.getvalue()
        self.assertIn("Total entries : 1", output)
        self.assertIn("Agent filter : Edit", output)


if __name__ == "__main__":
    unittest.main()
