#!/usr/bin/env python3
#
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0
#
"""Regression tests for prod-code agent guard hook (scripts/hooks/prod-code-local-build-guard.py)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

_SCRIPTS_DIR = str(Path(__file__).resolve().parent)
if _SCRIPTS_DIR not in sys.path:
    sys.path.insert(0, _SCRIPTS_DIR)

from prod_code_guard_cases.classification_and_build import (
    BuildCommandCasesMixin,
    ClassificationCasesMixin,
)
from prod_code_guard_cases.grep_interception import (
    GrepInterceptionCasesMixin,
)
from prod_code_guard_cases.rename_hook import (
    MultiFileRenameHookCasesMixin,
)
from prod_code_guard_cases.rename_unit import (
    MultiFileRenameUnitCasesMixin,
)
from prod_code_guard_cases.scripted_write_denials import (
    ScriptedSourceWriteDenialCasesMixin,
)
from prod_code_guard_cases.scripted_write_exemptions import (
    ScriptedSourceWriteExemptionCasesMixin,
)
from prod_code_guard_cases.sed_reads import SedSourceReadCasesMixin
from prod_code_guard_cases.support import (
    HOOK_SCRIPT,
    guard,
    make_git_repo_root,
    spec,
)
from prod_code_guard_cases.symbol_grep_policy import (
    SymbolGrepPolicyCasesMixin,
)

_test_git_repo_root = make_git_repo_root

__all__ = [
    "HOOK_SCRIPT",
    "ProdCodeGuardBuildCommandTests",
    "ProdCodeGuardClassificationTests",
    "ProdCodeGuardGrepInterceptionTests",
    "ProdCodeGuardMultiFileRenameTests",
    "ProdCodeGuardScriptedSourceWriteTests",
    "ProdCodeGuardSedSourceReadTests",
    "ProdCodeGuardSymbolGrepPolicyTests",
    "_test_git_repo_root",
    "guard",
    "spec",
]


class ProdCodeGuardClassificationTests(ClassificationCasesMixin, unittest.TestCase):
    pass


class ProdCodeGuardGrepInterceptionTests(GrepInterceptionCasesMixin, unittest.TestCase):
    pass


class ProdCodeGuardBuildCommandTests(BuildCommandCasesMixin, unittest.TestCase):
    pass


class ProdCodeGuardMultiFileRenameTests(
    MultiFileRenameUnitCasesMixin,
    MultiFileRenameHookCasesMixin,
    unittest.TestCase,
):
    pass


class ProdCodeGuardSymbolGrepPolicyTests(SymbolGrepPolicyCasesMixin, unittest.TestCase):
    pass


class ProdCodeGuardSedSourceReadTests(SedSourceReadCasesMixin, unittest.TestCase):
    pass


class ProdCodeGuardScriptedSourceWriteTests(
    ScriptedSourceWriteDenialCasesMixin,
    ScriptedSourceWriteExemptionCasesMixin,
    unittest.TestCase,
):
    pass


if __name__ == "__main__":
    unittest.main()
