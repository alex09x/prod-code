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

"""
package-deb.py - Builds a standard Debian (.deb) package for prod-code.
Pure Python standard library implementation: works on macOS and Linux without dpkg.
"""

import argparse
import io
import os
import sys
import tarfile
import time


def write_ar_header(name: str, size: int, mtime: int = 0, mode: int = 0o100644) -> bytes:
    """Creates a 60-byte GNU/Debian ar header."""
    # name: 16 bytes, left-justified, padded with spaces (Debian files don't need trailing slash if exactly matching)
    # mtime: 12 bytes
    # uid: 6 bytes
    # gid: 6 bytes
    # mode: 8 bytes octal
    # size: 10 bytes
    # fmag: 2 bytes: `\x60\n`
    header = (
        f"{name:<16}"
        f"{mtime:<12}"
        f"{'0':<6}"
        f"{'0':<6}"
        f"{oct(mode)[2:]:<8}"
        f"{size:<10}"
        "`\n"
    )
    return header.encode("ascii")


def create_control_tar(version: str, arch: str) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        # control file
        debian_arch = "arm64" if arch in ("aarch64", "arm64") else "amd64"
        control_content = (
            f"Package: prod-code\n"
            f"Version: {version}\n"
            f"Section: devel\n"
            f"Priority: optional\n"
            f"Architecture: {debian_arch}\n"
            f"Maintainer: Alexander Panasenko <alex@prod.codes>\n"
            f"Installed-Size: 75000\n"
            f"Homepage: https://prod.codes\n"
            f"Description: Remote Code Intelligence for AI coding agents and editors\n"
            f" prod-code mirrors your repository to cluster nodes with warm language\n"
            f" servers (rust-analyzer, gopls, clangd, basedpyright, typescript-language-server,\n"
            f" sourcekit-lsp) and runs builds, tests, refactorings and MCP tools remotely.\n"
        ).encode("utf-8")

        ti = tarfile.TarInfo(name="./control")
        ti.size = len(control_content)
        ti.mode = 0o644
        ti.mtime = int(time.time())
        tar.addfile(ti, io.BytesIO(control_content))

        # postinst
        postinst_content = (
            "#!/bin/sh\n"
            "set -e\n"
            "if [ -d /run/systemd/system ]; then\n"
            "    systemctl daemon-reload || true\n"
            "fi\n"
            "echo 'prod-code installed successfully. Run \"prod-code status\" or \"prod-code --help\".'\n"
            "exit 0\n"
        ).encode("utf-8")

        ti_post = tarfile.TarInfo(name="./postinst")
        ti_post.size = len(postinst_content)
        ti_post.mode = 0o755
        ti_post.mtime = int(time.time())
        tar.addfile(ti_post, io.BytesIO(postinst_content))

        # prerm
        prerm_content = (
            "#!/bin/sh\n"
            "set -e\n"
            "if [ -d /run/systemd/system ]; then\n"
            "    systemctl stop prod-code-gateway 2>/dev/null || true\n"
            "fi\n"
            "exit 0\n"
        ).encode("utf-8")

        ti_prerm = tarfile.TarInfo(name="./prerm")
        ti_prerm.size = len(prerm_content)
        ti_prerm.mode = 0o755
        ti_prerm.mtime = int(time.time())
        tar.addfile(ti_prerm, io.BytesIO(prerm_content))

    return buf.getvalue()


def create_data_tar(
    client_bin: str,
    server_bin: str = None,
    service_file: str = None,
    readme_file: str = None,
) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        # Directories
        for d in [
            "./usr",
            "./usr/bin",
            "./usr/share",
            "./usr/share/doc",
            "./usr/share/doc/prod-code",
            "./lib",
            "./lib/systemd",
            "./lib/systemd/system",
        ]:
            ti = tarfile.TarInfo(name=d)
            ti.type = tarfile.DIRTYPE
            ti.mode = 0o755
            ti.mtime = int(time.time())
            tar.addfile(ti)

        # /usr/bin/prod-code
        if os.path.exists(client_bin):
            with open(client_bin, "rb") as f:
                content = f.read()
            ti = tarfile.TarInfo(name="./usr/bin/prod-code")
            ti.size = len(content)
            ti.mode = 0o755
            ti.mtime = int(time.time())
            tar.addfile(ti, io.BytesIO(content))

        # /usr/bin/prod-code-server
        if server_bin and os.path.exists(server_bin):
            with open(server_bin, "rb") as f:
                content = f.read()
            ti = tarfile.TarInfo(name="./usr/bin/prod-code-server")
            ti.size = len(content)
            ti.mode = 0o755
            ti.mtime = int(time.time())
            tar.addfile(ti, io.BytesIO(content))

        # systemd service
        if service_file and os.path.exists(service_file):
            with open(service_file, "rb") as f:
                content = f.read()
            ti = tarfile.TarInfo(name="./lib/systemd/system/prod-code-gateway.service")
            ti.size = len(content)
            ti.mode = 0o644
            ti.mtime = int(time.time())
            tar.addfile(ti, io.BytesIO(content))

        # readme/doc
        if readme_file and os.path.exists(readme_file):
            with open(readme_file, "rb") as f:
                content = f.read()
            ti = tarfile.TarInfo(name="./usr/share/doc/prod-code/README.md")
            ti.size = len(content)
            ti.mode = 0o644
            ti.mtime = int(time.time())
            tar.addfile(ti, io.BytesIO(content))

    return buf.getvalue()


def assemble_deb(out_path: str, control_tar: bytes, data_tar: bytes) -> None:
    debian_binary = b"2.0\n"

    with open(out_path, "wb") as f:
        # ar file magic signature
        f.write(b"!<arch>\n")

        # 1. debian-binary
        f.write(write_ar_header("debian-binary", len(debian_binary)))
        f.write(debian_binary)
        if len(debian_binary) % 2 != 0:
            f.write(b"\n")

        # 2. control.tar.gz
        f.write(write_ar_header("control.tar.gz", len(control_tar)))
        f.write(control_tar)
        if len(control_tar) % 2 != 0:
            f.write(b"\n")

        # 3. data.tar.gz
        f.write(write_ar_header("data.tar.gz", len(data_tar)))
        f.write(data_tar)
        if len(data_tar) % 2 != 0:
            f.write(b"\n")


def main():
    parser = argparse.ArgumentParser(description="Build Debian package for prod-code")
    parser.add_argument("--version", required=True, help="Package version (e.g. 0.3.19)")
    parser.add_argument("--arch", required=True, choices=["x86_64", "amd64", "aarch64", "arm64"])
    parser.add_argument("--client-bin", required=True, help="Path to prod-code client binary")
    parser.add_argument("--server-bin", help="Path to prod-code-server binary")
    parser.add_argument("--service-file", help="Path to systemd service unit")
    parser.add_argument("--readme", help="Path to README.md")
    parser.add_argument("--output", required=True, help="Output .deb file path")

    args = parser.parse_args()

    control_tar = create_control_tar(args.version, args.arch)
    data_tar = create_data_tar(
        args.client_bin, args.server_bin, args.service_file, args.readme
    )

    assemble_deb(args.output, control_tar, data_tar)
    print(f"Successfully generated Debian package: {args.output} ({os.path.getsize(args.output)} bytes)")


if __name__ == "__main__":
    main()
