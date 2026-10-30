# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Cohesive test-case mixins and shared fixtures for scripts/test_prod_code_guard.py.

Concrete ``unittest.TestCase`` subclasses are defined exclusively in
``scripts/test_prod_code_guard.py`` so public unittest class paths stay stable and
``unittest discover`` executes each guard test once.
"""

CASE_SUBMODULES = (
    "__init__.py",
    "support.py",
    "classification_and_build.py",
    "grep_interception.py",
    "rename_unit.py",
    "rename_hook.py",
    "symbol_grep_policy.py",
    "sed_reads.py",
    "scripted_write_denials.py",
    "scripted_write_exemptions.py",
)
