# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Modular components for the prod-code PreToolUse agent guard hook."""

GUARD_SUBMODULES = (
    "__init__.py",
    "shell.py",
    "grep_patterns.py",
    "grep_guard.py",
    "sed_guard.py",
    "shell_lex.py",
    "shell_python.py",
    "python_write_eval.py",
    "python_write_guard.py",
    "payload.py",
    "rename_guard.py",
    "dispatch.py",
)
