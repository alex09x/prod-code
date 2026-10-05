#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

import os
import sys
import unittest
from unittest.mock import patch, MagicMock

# Add scripts directory to sys.path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import importlib
cluster_rollout = importlib.import_module("cluster-rollout")


class TestClusterRollout(unittest.TestCase):
    def test_version_retrieval(self):
        repo_root = cluster_rollout.get_repo_root()
        ver = cluster_rollout.get_current_version(repo_root)
        self.assertRegex(ver, r"^\d+\.\d+\.\d+")

    def test_node_alias_mapping(self):
        self.assertEqual(cluster_rollout.NODE_ALIASES.get("192.168.2.143"), "ram9")
        self.assertEqual(cluster_rollout.NODE_ALIASES.get("192.168.2.168"), "booster")
        self.assertEqual(cluster_rollout.NODE_ALIASES.get("192.168.2.40"), "macbook")

    def test_local_binary_path_lookup(self):
        repo_root = cluster_rollout.get_repo_root()
        ver = cluster_rollout.get_current_version(repo_root)
        bin_path = cluster_rollout.get_local_binary_path(repo_root, "linux", "x86_64", ver)
        self.assertTrue(bin_path is None or os.path.isfile(bin_path))

    def test_dry_run_linux_deployment(self):
        ok, msg = cluster_rollout.deploy_linux_node_fast(
            "192.168.2.143", "/dummy/prod-code-server", "0.3.24", dry_run=True
        )
        self.assertTrue(ok)
        self.assertIn("[DRY-RUN]", msg)

    def test_dry_run_mac_deployment(self):
        repo_root = cluster_rollout.get_repo_root()
        ok, msg = cluster_rollout.deploy_mac_node(
            "192.168.2.40", repo_root, "/dummy/prod-code-server", "0.3.24", dry_run=True
        )
        self.assertTrue(ok)
        self.assertIn("[DRY-RUN]", msg)

    def test_dry_run_client_update(self):
        repo_root = cluster_rollout.get_repo_root()
        ver = cluster_rollout.get_current_version(repo_root)
        ok, msg = cluster_rollout.update_local_client(repo_root, ver, dry_run=True)
        self.assertTrue(ok)
        self.assertIn("[DRY-RUN]", msg)


if __name__ == "__main__":
    unittest.main()
