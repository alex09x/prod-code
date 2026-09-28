#!/usr/bin/env python3
"""Regression tests for the remote build shell embedded in deploy-mac-node.sh."""

from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("deploy-mac-node.sh")
REMOTE_BUILD = re.compile(r"REMOTE_BUILD=\$\(cat <<'RB'\n(.*?)\nRB\n\)", re.DOTALL)


class RemoteBuildFailureTests(unittest.TestCase):
    def test_cargo_failure_survives_tail_pipeline(self) -> None:
        match = REMOTE_BUILD.search(SCRIPT.read_text(encoding="utf-8"))
        self.assertIsNotNone(match, "remote build block must remain extractable for this test")

        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            cargo = home / ".cargo" / "bin" / "cargo"
            cargo.parent.mkdir(parents=True)
            cargo.write_text(
                "#!/bin/sh\necho simulated compiler failure >&2\nexit 23\n",
                encoding="utf-8",
            )
            cargo.chmod(0o755)
            (home / "prod-code").mkdir()

            environment = dict(os.environ, HOME=str(home), PROFILE="dev")
            result = subprocess.run(
                ["bash", "-c", match.group(1)],
                cwd=home,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
        self.assertNotIn("target/debug/prod-code-server", result.stdout)


if __name__ == "__main__":
    unittest.main()
