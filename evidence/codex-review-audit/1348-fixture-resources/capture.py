# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Capture one foreground gate without publishing machine-specific paths."""
import json
from pathlib import Path
import subprocess
import sys

prefix, *command = sys.argv[1:]
result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
output = result.stdout.decode(errors="replace").replace(str(Path.cwd()), "<remote-checkout>")
Path(prefix + ".txt").write_text(output)
Path(prefix + ".json").write_text(json.dumps({"command": command, "exit": result.returncode}, indent=2) + "\n")
print(output)
print(f"terminal exit: {result.returncode}")
sys.exit(result.returncode)
