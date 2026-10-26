#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Run the two directly affected, non-overlapping integration scopes."""

import subprocess

commands = [
    ["cargo", "test", "-p", "prod-code-mcp", "--test", "rust_method_preservation"],
    [
        "cargo", "test", "-p", "prod-code-mcp", "--test", "orchestration",
        "an_associated_function_becomes_a_method_with_every_call", "--", "--exact",
    ],
]
for command in commands:
    result = subprocess.run(command, check=False)
    if result.returncode:
        raise SystemExit(result.returncode)
