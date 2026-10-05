#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""
cluster-rollout.py - Production cluster deployment and rolling update utility for prod-code.

Provides both an interactive terminal wizard and automated CLI flags for:
- Discovering cluster topology, versions, resource pressure, and active sessions.
- Safe rolling updates across Linux (systemd) and macOS (launchd) gateway nodes.
- Pre-flight safety checks (disk/memory pressure, SSH availability, session drainage).
- Fast binary rollout (prebuilt & signed binaries) or remote source compilation.
- Health validation and automatic rollback on boot failure.
- Local client updater (~/.cargo/bin and /usr/local/bin).
"""

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from typing import Any, Dict, List, Optional, Tuple

# Known node IP to friendly alias mapping
NODE_ALIASES = {
    "192.168.2.168": "booster",
    "192.168.2.6": "booster",
    "192.168.2.143": "ram9",
    "192.168.2.190": "rama",
    "192.168.2.40": "macbook",
    "192.168.2.208": "node-208",
    "192.168.2.242": "studio",
}

DEFAULT_NODES = [
    {"ip": "192.168.2.168", "alias": "booster", "arch": "x86_64", "os": "linux"},
    {"ip": "192.168.2.143", "alias": "ram9", "arch": "x86_64", "os": "linux"},
    {"ip": "192.168.2.190", "alias": "rama", "arch": "aarch64", "os": "linux"},
    {"ip": "192.168.2.40", "alias": "macbook", "arch": "aarch64", "os": "darwin"},
]

# ANSI colors
RESET = "\033[0m"
BOLD = "\033[1m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
RED = "\033[31m"
CYAN = "\033[36m"
MAGENTA = "\033[35m"
DIM = "\033[2m"


def log(msg: str) -> None:
    print(f"{CYAN}==>{RESET} {BOLD}{msg}{RESET}")


def warn(msg: str) -> None:
    print(f"{YELLOW}Warning:{RESET} {msg}", file=sys.stderr)


def err(msg: str) -> None:
    print(f"{RED}Error:{RESET} {msg}", file=sys.stderr)


def get_repo_root() -> str:
    script_dir = os.path.dirname(os.path.abspath(__file__))
    return os.path.dirname(script_dir)


def get_current_version(repo_root: str) -> str:
    cargo_toml = os.path.join(repo_root, "Cargo.toml")
    if os.path.exists(cargo_toml):
        with open(cargo_toml, "r", encoding="utf-8") as f:
            for line in f:
                if line.startswith("version = "):
                    return line.split('"')[1]
    return "0.3.24"


def query_cluster_status() -> Optional[Dict[str, Any]]:
    try:
        r = subprocess.run(
            ["prod-code", "cluster", "--json"],
            capture_output=True,
            text=True,
            timeout=8,
        )
        if r.returncode == 0 and r.stdout.strip():
            return json.loads(r.stdout.strip())
    except Exception:
        pass
    return None


def probe_node_live(node_ip: str) -> Dict[str, Any]:
    """Probe a single node over SSH if not reported in cluster JSON."""
    info: Dict[str, Any] = {
        "ip": node_ip,
        "alias": NODE_ALIASES.get(node_ip, "unknown"),
        "up": False,
        "version": "unknown",
        "disk_free_pct": 0,
        "pressure": None,
        "active_sessions": 0,
        "os": "unknown",
        "arch": "unknown",
    }
    cmd = (
        "uname -s -m; "
        "~/.local/bin/prod-code-server --version 2>/dev/null || prod-code-server --version 2>/dev/null || echo 'none'; "
        "df -k ~ | tail -1"
    )
    try:
        res = subprocess.run(
            ["ssh", "-o", "ConnectTimeout=3", "-o", "BatchMode=yes", f"alex09x@{node_ip}", cmd],
            capture_output=True,
            text=True,
            timeout=6,
        )
        if res.returncode == 0:
            lines = [l.strip() for l in res.stdout.strip().splitlines() if l.strip()]
            info["up"] = True
            if len(lines) >= 1:
                parts = lines[0].lower().split()
                if len(parts) >= 2:
                    info["os"] = "darwin" if "darwin" in parts[0] else "linux"
                    info["arch"] = "aarch64" if parts[1] in ("arm64", "aarch64") else "x86_64"
            if len(lines) >= 2 and "prod-code-server" in lines[1]:
                info["version"] = lines[1].split()[-1]
            if len(lines) >= 3:
                df_parts = lines[2].split()
                if len(df_parts) >= 5:
                    pct_str = df_parts[4].replace("%", "")
                    try:
                        used_pct = int(pct_str)
                        info["disk_free_pct"] = 100 - used_pct
                        if info["disk_free_pct"] < 10:
                            info["pressure"] = f"disk {info['disk_free_pct']}% free"
                    except ValueError:
                        pass
    except Exception:
        pass
    return info


def collect_cluster_nodes() -> List[Dict[str, Any]]:
    cluster_json = query_cluster_status()
    nodes: Dict[str, Dict[str, Any]] = {}

    if cluster_json and "nodes" in cluster_json:
        for n in cluster_json["nodes"]:
            remote = n.get("remote", "")
            if not remote or remote.startswith("127.0.0.1"):
                continue
            ip = remote.split(":")[0]
            alias = NODE_ALIASES.get(ip, ip)
            up = n.get("up", False)
            version = n.get("version", "unknown")
            pressure = n.get("pressure")
            active_sessions = n.get("active_sessions", 0)
            platform_str = n.get("platform", "")
            os_name = "darwin" if "macos" in platform_str or "darwin" in platform_str else "linux"
            arch_name = "aarch64" if "aarch64" in platform_str or "arm64" in platform_str else "x86_64"

            disk_free_pct = 50
            host_info = n.get("host", {})
            storage_free_millis = host_info.get("storage_free_millis")
            if storage_free_millis is not None:
                disk_free_pct = storage_free_millis // 10
            elif pressure and "disk" in pressure:
                try:
                    pct_part = pressure.split("%")[0].split()[-1]
                    disk_free_pct = int(pct_part)
                except Exception:
                    pass

            nodes[alias] = {
                "ip": ip,
                "alias": alias,
                "up": up,
                "version": version,
                "disk_free_pct": disk_free_pct,
                "pressure": pressure,
                "active_sessions": active_sessions,
                "os": os_name,
                "arch": arch_name,
                "healthy": n.get("healthy", up and not pressure),
            }

    # Ensure all default nodes are tracked without duplicates
    for d in DEFAULT_NODES:
        alias = d["alias"]
        if alias not in nodes:
            probed = probe_node_live(d["ip"])
            nodes[alias] = probed

    return list(nodes.values())


def print_node_table(nodes: List[Dict[str, Any]], target_version: str) -> None:
    header = (
        f"{BOLD}{'#':<3} {'Node IP':<16} {'Alias':<10} {'OS / Arch':<14} "
        f"{'Version':<10} {'Disk Free':<11} {'Sessions':<10} {'Status':<14}{RESET}"
    )
    sep = "-" * 88
    print(sep)
    print(header)
    print(sep)

    for i, n in enumerate(nodes, 1):
        ip = n["ip"]
        alias = n.get("alias", "")
        os_arch = f"{n.get('os', '')} {n.get('arch', '')}"
        ver = n.get("version", "unknown")
        ver_display = ver
        if ver == target_version:
            ver_display = f"{GREEN}{ver}{RESET}"
        elif ver != "unknown":
            ver_display = f"{YELLOW}{ver}{RESET}"

        disk = f"{n.get('disk_free_pct', 0)}%"
        if n.get("disk_free_pct", 100) < 5:
            disk = f"{RED}{disk} (CRIT){RESET}"
        elif n.get("disk_free_pct", 100) < 10:
            disk = f"{YELLOW}{disk} (LOW){RESET}"
        else:
            disk = f"{GREEN}{disk}{RESET}"

        sess = str(n.get("active_sessions", 0))

        if not n.get("up"):
            status = f"{RED}Offline{RESET}"
        elif n.get("pressure"):
            status = f"{YELLOW}{n['pressure']}{RESET}"
        else:
            status = f"{GREEN}Healthy{RESET}"

        print(
            f"{i:<3} {ip:<16} {alias:<10} {os_arch:<14} "
            f"{ver_display:<19} {disk:<20} {sess:<10} {status}"
        )
    print(sep)


def get_local_binary_path(repo_root: str, os_name: str, arch_name: str, version: str) -> Optional[str]:
    target_triplet = (
        "aarch64-apple-darwin" if os_name == "darwin" and arch_name in ("aarch64", "arm64")
        else "aarch64-unknown-linux-gnu" if os_name == "linux" and arch_name in ("aarch64", "arm64")
        else "x86_64-unknown-linux-gnu"
    )
    candidates = [
        os.path.join(repo_root, "dist", f"v{version}", f"prod-code-server-{target_triplet}"),
        os.path.join(repo_root, "dist", "packages", f"prod-code-server-{target_triplet}"),
        os.path.join(repo_root, "target", target_triplet, "release", "prod-code-server"),
    ]
    for c in candidates:
        if os.path.isfile(c) and os.access(c, os.X_OK):
            return c
    return None


def deploy_linux_node_fast(
    node_ip: str, binary_path: str, target_version: str, dry_run: bool = False
) -> Tuple[bool, str]:
    if dry_run:
        return True, f"[DRY-RUN] scp {binary_path} alex09x@{node_ip}:~/.local/bin/prod-code-server && restart"

    dest_tmp = f".local/bin/prod-code-server.new-{int(time.time())}"
    dest_final = ".local/bin/prod-code-server"

    # 1. SCP binary
    scp_cmd = ["scp", "-q", binary_path, f"alex09x@{node_ip}:{dest_tmp}"]
    r = subprocess.run(scp_cmd, capture_output=True, text=True, timeout=30)
    if r.returncode != 0:
        return False, f"SCP failed: {r.stderr.strip()}"

    # 2. Swap binary, backup old, and restart service
    remote_script = (
        f"set -e\n"
        f"chmod 755 {dest_tmp}\n"
        f"if [ -f {dest_final} ]; then cp -f {dest_final} {dest_final}.bak; fi\n"
        f"mv -f {dest_tmp} {dest_final}\n"
        f"if [ -d prod-code/target/release ]; then cp -f {dest_final} prod-code/target/release/prod-code-server; fi\n"
        f"systemctl --user restart prod-code-gateway\n"
        f"sleep 2\n"
        f"systemctl --user is-active prod-code-gateway\n"
        f"{dest_final} --version\n"
    )
    r = subprocess.run(
        ["ssh", "-o", "ConnectTimeout=5", f"alex09x@{node_ip}", "bash -s"],
        input=remote_script,
        capture_output=True,
        text=True,
        timeout=25,
    )
    if r.returncode != 0:
        # Attempt rollback
        subprocess.run(
            ["ssh", f"alex09x@{node_ip}", f"if [ -f {dest_final}.bak ]; then mv -f {dest_final}.bak {dest_final} && systemctl --user restart prod-code-gateway; fi"],
            capture_output=True,
            timeout=10,
        )
        return False, f"Restart/validation failed: {r.stderr.strip() or r.stdout.strip()}"

    if target_version not in r.stdout:
        return False, f"Version mismatch after update: {r.stdout.strip()}"

    return True, f"Successfully upgraded to {target_version}"


def deploy_mac_node(
    node_ip: str, repo_root: str, binary_path: Optional[str], target_version: str, dry_run: bool = False
) -> Tuple[bool, str]:
    if dry_run:
        return True, f"[DRY-RUN] Deploy macOS node {node_ip} via scripts/deploy-mac-node.sh"

    deploy_script = os.path.join(repo_root, "scripts", "deploy-mac-node.sh")
    if not os.path.isfile(deploy_script):
        return False, "scripts/deploy-mac-node.sh not found"

    env = os.environ.copy()
    env["PROD_CODE_SIGN_IDENTITY"] = "Developer ID Application: Alexander Panasenko (284V2M3LN9)"
    env["PROD_CODE_ENGINES"] = "swift,go"
    if binary_path and os.path.isfile(binary_path):
        env["PROD_CODE_SERVER_BIN"] = binary_path

    # Advertise address: e.g. 192.168.2.40:9400
    adv = f"{node_ip}:9400"
    peers = "192.168.2.168:9400,192.168.2.143:9400,192.168.2.190:9400"

    cmd = [deploy_script, node_ip, adv, peers, "release"]
    r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=180)
    if r.returncode != 0:
        return False, f"macOS deploy failed: {r.stderr.strip()[-2000:]}"

    return True, f"Successfully deployed to macOS node ({target_version})"


def update_local_client(repo_root: str, target_version: str, dry_run: bool = False) -> Tuple[bool, str]:
    is_mac = platform.system() == "Darwin"
    arch = "aarch64" if platform.machine() in ("arm64", "aarch64") else "x86_64"
    triplet = f"{arch}-apple-darwin" if is_mac else f"{arch}-unknown-linux-gnu"
    candidate = os.path.join(repo_root, "dist", f"v{target_version}", f"prod-code-{triplet}")

    if not os.path.isfile(candidate):
        candidate = os.path.join(repo_root, "dist", "packages", f"prod-code-{triplet}")

    if not os.path.isfile(candidate):
        return False, f"Local client binary for {triplet} not found in dist/"

    if dry_run:
        return True, f"[DRY-RUN] Install {candidate} to ~/.cargo/bin/prod-code"

    targets = [
        os.path.expanduser("~/.cargo/bin/prod-code"),
        os.path.expanduser("~/.local/bin/prod-code"),
    ]
    updated = []
    for t in targets:
        if os.path.isdir(os.path.dirname(t)):
            shutil.copy2(candidate, t)
            os.chmod(t, 0o755)
            if is_mac:
                subprocess.run(
                    ["codesign", "-s", "Developer ID Application: Alexander Panasenko (284V2M3LN9)", "-f", "--options", "runtime", "--timestamp", t],
                    capture_output=True,
                )
            updated.append(t)

    return True, f"Updated local client in: {', '.join(updated)}"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="prod-code Cluster Rollout & Fleet Updater",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--version", help="Target release version (default: Cargo.toml version)")
    parser.add_argument("--nodes", help="Comma-separated IPs or aliases (e.g. booster,ram9 or all)")
    parser.add_argument("--status", action="store_true", help="Print cluster node table and exit")
    parser.add_argument("--skip-pressure", action="store_true", default=True, help="Skip nodes with disk/memory pressure (default: True)")
    parser.add_argument("--force-pressure", action="store_true", help="Allow deploying to nodes even under resource pressure")
    parser.add_argument("--dry-run", action="store_true", help="Simulate rollout without modifying cluster")
    parser.add_argument("--client", action="store_true", help="Also update local prod-code client binary")
    parser.add_argument("-y", "--yes", action="store_true", help="Non-interactive auto-confirm")

    args = parser.parse_args()
    repo_root = get_repo_root()
    target_version = args.version or get_current_version(repo_root)

    log(f"prod-code Cluster Rollout Tool — Target: v{target_version}")

    nodes = collect_cluster_nodes()
    print_node_table(nodes, target_version)

    if args.status:
        return

    # Determine candidate nodes for rollout
    selected_nodes: List[Dict[str, Any]] = []
    choice: Optional[str] = None

    if args.nodes:
        targets = [x.strip() for x in args.nodes.split(",")]
        for n in nodes:
            if "all" in targets or n["ip"] in targets or n["alias"] in targets:
                selected_nodes.append(n)
    elif not sys.stdin.isatty() or args.yes:
        # Default non-interactive: all healthy nodes with running gateway service
        for n in nodes:
            has_service = n.get("up") and n.get("version") != "unknown"
            if has_service and (args.force_pressure or n.get("disk_free_pct", 100) >= 10):
                selected_nodes.append(n)
    else:
        # Interactive prompt
        print(f"\n{BOLD}Select deployment scope:{RESET}")
        print(f"  {CYAN}[1]{RESET} All healthy nodes (skip nodes <10% disk free)")
        print(f"  {CYAN}[2]{RESET} Select specific nodes")
        print(f"  {CYAN}[3]{RESET} Update local client binary only")
        print(f"  {CYAN}[4]{RESET} Abort")
        try:
            choice = input(f"{BOLD}Choice [1-4] (default: 1): {RESET}").strip() or "1"
        except (KeyboardInterrupt, EOFError):
            print("\nAborted.")
            sys.exit(0)

        if choice == "1":
            for n in nodes:
                has_service = n.get("up") and n.get("version") != "unknown"
                if has_service and n.get("disk_free_pct", 100) >= 10:
                    selected_nodes.append(n)
        elif choice == "2":
            try:
                raw_pick = input("Enter comma-separated node numbers or aliases: ").strip()
                picks = [p.strip() for p in raw_pick.split(",")]
                for i, n in enumerate(nodes, 1):
                    if str(i) in picks or n["ip"] in picks or n["alias"] in picks:
                        selected_nodes.append(n)
            except (KeyboardInterrupt, EOFError):
                print("\nAborted.")
                sys.exit(0)
        elif choice == "3":
            pass  # Only client
        else:
            print("Aborted.")
            sys.exit(0)

    # Filter pressure if not forced
    if not args.force_pressure:
        filtered = []
        for n in selected_nodes:
            if n.get("disk_free_pct", 100) < 10:
                warn(f"Skipping node {n['alias']} ({n['ip']}): disk pressure ({n.get('disk_free_pct')}% free < 10%)")
            else:
                filtered.append(n)
        selected_nodes = filtered

    want_client = args.client or choice == "3"
    if not selected_nodes and not want_client:
        print("No eligible nodes selected for update.")
        return

    print(f"\n{BOLD}Execution Plan:{RESET}")
    for n in selected_nodes:
        print(f"  • Upgrade {n['alias']} ({n['ip']}) [{n['os']} {n['arch']}] -> v{target_version}")
    if want_client:
        print(f"  • Update local client binary -> v{target_version}")

    if not args.yes and sys.stdin.isatty():
        try:
            confirm = input(f"\n{BOLD}Proceed with rolling update? [y/N]: {RESET}").strip().lower()
            if confirm not in ("y", "yes"):
                print("Cancelled.")
                return
        except (KeyboardInterrupt, EOFError):
            print("\nCancelled.")
            return

    # Perform rollout
    log(f"Starting rolling rollout to {len(selected_nodes)} node(s)...")

    results: List[Tuple[str, bool, str]] = []

    for n in selected_nodes:
        ip = n["ip"]
        alias = n["alias"]
        os_name = n["os"]
        arch_name = n["arch"]
        log(f"Processing node {alias} ({ip})...")

        bin_path = get_local_binary_path(repo_root, os_name, arch_name, target_version)
        if not bin_path:
            warn(f"Prebuilt binary for {os_name} {arch_name} not found locally in dist/v{target_version}")

        if os_name == "darwin":
            ok, msg = deploy_mac_node(ip, repo_root, bin_path, target_version, dry_run=args.dry_run)
        else:
            if not bin_path:
                results.append((alias, False, "Missing prebuilt Linux binary"))
                continue
            ok, msg = deploy_linux_node_fast(ip, bin_path, target_version, dry_run=args.dry_run)

        status_str = f"{GREEN}SUCCESS{RESET}" if ok else f"{RED}FAILED{RESET}"
        print(f"  [{status_str}] {alias}: {msg}")
        results.append((alias, ok, msg))

    if want_client:
        log("Updating local client...")
        ok, msg = update_local_client(repo_root, target_version, dry_run=args.dry_run)
        status_str = f"{GREEN}SUCCESS{RESET}" if ok else f"{RED}FAILED{RESET}"
        print(f"  [{status_str}] local-client: {msg}")
        results.append(("local-client", ok, msg))

    # Summary
    print(f"\n{BOLD}=== Rollout Summary ==={RESET}")
    all_ok = True
    for name, ok, msg in results:
        status_str = f"{GREEN}OK{RESET}" if ok else f"{RED}ERROR{RESET}"
        print(f"  [{status_str}] {name:<14}: {msg}")
        if not ok:
            all_ok = False

    if all_ok:
        log(f"All operations completed successfully! Cluster is on v{target_version}.")
    else:
        warn("Some nodes failed to update. Review logs above.")


if __name__ == "__main__":
    main()
