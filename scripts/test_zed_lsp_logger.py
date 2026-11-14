#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Acceptance of editor protocol transparency, bounded logs and termination."""

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

LOGGER = Path(__file__).with_name("zed-lsp-logger.py")


class LoggerTests(unittest.TestCase):
    def fixture(self, root, body):
        binary = root / "bridge"
        binary.write_text(
            "#!/usr/bin/env python3\nimport os,sys,time,signal\n"
            "if '--version' in sys.argv:\n print('fixture 1'); sys.exit(0)\n" + body
        )
        binary.chmod(0o755)
        return [
            sys.executable,
            str(LOGGER),
            "--binary",
            str(binary),
            "--language",
            "rust",
            "--log-dir",
            str(root / "logs"),
            "--max-log-bytes",
            "1024",
        ]

    def test_protocol_bytes_exit_code_trace_and_rotation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            command = self.fixture(
                root,
                "with open(os.environ['PROD_CODE_LSP_TRACE'],'w') as f:\n"
                " f.write('123 reconnect restored attempt=1\\n'); f.flush()\n"
                "data=sys.stdin.buffer.read(); sys.stdout.buffer.write(data)\n"
                "sys.stderr.buffer.write(b'failure detail\\n'*3000)\n"
                "time.sleep(.3); sys.exit(7)\n",
            )
            payload = b'{"jsonrpc":"2.0","id":1}\r\n\x00'
            result = subprocess.run(
                command, input=payload, capture_output=True, timeout=10, check=False
            )
            self.assertEqual(result.returncode, 7)
            self.assertEqual(result.stdout, payload)
            self.assertEqual(result.stderr, b"failure detail\n" * 3000)
            session = next((root / "logs").iterdir())
            meta = json.loads((session / "session.json").read_text())
            self.assertEqual(meta["exit_code"], 7)
            self.assertEqual(meta["version"], "fixture 1")
            self.assertIn("reconnect restored", (session / "transport.log").read_text())
            logs = list(session.glob("stderr.log*"))
            self.assertEqual(len(logs), 4)
            self.assertTrue(all(p.stat().st_size < 6000 for p in logs))
            self.assertTrue(
                all(p.stat().st_mode & 0o077 == 0 for p in session.iterdir())
            )
            self.assertFalse((session / "trace.pipe").exists())

    def test_launchers_started_together_prune_the_same_sessions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            command = self.fixture(root, "sys.exit(0)\n")
            logs = root / "logs"
            for index in range(60):
                finished = logs / f"session-{index}-rust-{index}"
                finished.mkdir(parents=True)
                (finished / "session.json").write_text(
                    json.dumps({"logger": "prod-code-zed", "exit_code": 0})
                )
                (finished / "stderr.log").write_text("x" * 4096)
            launchers = [
                subprocess.Popen(
                    command, stdin=subprocess.DEVNULL, stderr=subprocess.PIPE
                )
                for _ in range(8)
            ]
            results = [(p.communicate(timeout=20)[1], p.returncode) for p in launchers]
            for stderr, code in results:
                self.assertEqual(code, 0, stderr.decode(errors="replace"))
            self.assertLessEqual(len(list(logs.glob("session-*"))), 20 + 8)

    def test_a_binary_named_without_a_path_is_found_on_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            command = self.fixture(root, "sys.exit(0)\n")
            command[command.index("--binary") + 1] = "bridge"
            env = dict(os.environ, PATH=f"{root}{os.pathsep}{os.environ['PATH']}")
            result = subprocess.run(
                command,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                timeout=10,
                check=False,
                env=env,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            session = next((root / "logs").iterdir())
            meta = json.loads((session / "session.json").read_text())
            self.assertNotIn("version_probe_error", meta)
            self.assertEqual(meta["binary_inode"], (root / "bridge").stat().st_ino)

    @unittest.skipUnless(os.name == "posix", "editor launcher uses POSIX signals")
    def test_editor_termination_reaches_bridge(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            marker = root / "terminated"
            command = self.fixture(
                root,
                "def stop(n,f):\n"
                f" open({str(marker)!r},'w').write('stopped'); sys.exit(0)\n"
                "signal.signal(signal.SIGTERM,stop)\n"
                "with open(os.environ['PROD_CODE_LSP_TRACE'],'w') as f: f.write('ready\\n')\n"
                "while True: time.sleep(.1)\n",
            )
            process = subprocess.Popen(
                command,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    traces = list((root / "logs").glob("session-*/transport.log"))
                    if traces and "ready" in traces[0].read_text():
                        break
                    time.sleep(0.02)
                else:
                    self.fail("bridge did not become ready")
                process.terminate()
                process.communicate(timeout=5)
                self.assertEqual(process.returncode, 0)
                self.assertEqual(marker.read_text(), "stopped")
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)


if __name__ == "__main__":
    unittest.main()
