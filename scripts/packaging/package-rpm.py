#!/usr/bin/env python3
"""
package-rpm.py - Builds a standard RPM (.rpm) package for prod-code.
Pure Python standard library implementation: runs on macOS and Linux without rpmbuild.
"""

import argparse
import hashlib
import io
import gzip
import os
import struct
import sys
import time
from typing import List, Tuple, Dict, Any


def make_cpio_entry(path: str, data: bytes, mode: int, mtime: int, ino: int) -> bytes:
    """Creates a single SVR4 cpio newc entry."""
    # Prefix relative path with ./ for standard RPM payload convention
    if not path.startswith("./"):
        rel_path = "./" + path.lstrip("/")
    else:
        rel_path = path

    path_bytes = rel_path.encode("utf-8") + b"\x00"
    namesize = len(path_bytes)
    filesize = len(data)

    header = (
        b"070701"                               # magic
        + f"{ino:08X}".encode("ascii")          # ino
        + f"{mode:08X}".encode("ascii")         # mode
        + f"{0:08X}".encode("ascii")            # uid (0 = root)
        + f"{0:08X}".encode("ascii")            # gid (0 = root)
        + f"{1:08X}".encode("ascii")            # nlink
        + f"{mtime:08X}".encode("ascii")        # mtime
        + f"{filesize:08X}".encode("ascii")     # filesize
        + f"{0:08X}".encode("ascii")            # devmajor
        + f"{0:08X}".encode("ascii")            # devminor
        + f"{0:08X}".encode("ascii")            # rdevmajor
        + f"{0:08X}".encode("ascii")            # rdevminor
        + f"{namesize:08X}".encode("ascii")     # namesize
        + f"{0:08X}".encode("ascii")            # check
    )

    pad_name = (4 - ((len(header) + namesize) % 4)) % 4
    pad_data = (4 - (filesize % 4)) % 4

    return header + path_bytes + (b"\x00" * pad_name) + data + (b"\x00" * pad_data)


def make_cpio_trailer() -> bytes:
    """Creates SVR4 cpio TRAILER!!! entry."""
    path_bytes = b"TRAILER!!!\x00"
    namesize = len(path_bytes)
    header = (
        b"070701"
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{1:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
        + f"{namesize:08X}".encode("ascii")
        + f"{0:08X}".encode("ascii")
    )
    pad_name = (4 - ((len(header) + namesize) % 4)) % 4
    return header + path_bytes + (b"\x00" * pad_name)


class RpmHeaderBuilder:
    """Builds binary RPM header structure with index and data sections."""

    RPM_NULL = 0
    RPM_CHAR = 1
    RPM_INT8 = 2
    RPM_INT16 = 3
    RPM_INT32 = 4
    RPM_INT64 = 5
    RPM_STRING = 6
    RPM_BIN = 7
    RPM_STRING_ARRAY = 8
    RPM_I18NSTRING = 9

    def __init__(self):
        self.entries: List[Tuple[int, int, bytes, int]] = []

    def add_string(self, tag: int, val: str):
        data = val.encode("utf-8") + b"\x00"
        self.entries.append((tag, self.RPM_STRING, data, 1))

    def add_string_array(self, tag: int, vals: List[str]):
        data = b"".join(v.encode("utf-8") + b"\x00" for v in vals)
        self.entries.append((tag, self.RPM_STRING_ARRAY, data, len(vals)))

    def add_i18n_string(self, tag: int, val: str):
        data = val.encode("utf-8") + b"\x00"
        self.entries.append((tag, self.RPM_I18NSTRING, data, 1))

    def add_int16_array(self, tag: int, vals: List[int]):
        data = struct.pack(f"!{len(vals)}H", *vals)
        self.entries.append((tag, self.RPM_INT16, data, len(vals)))

    def add_int32(self, tag: int, val: int):
        data = struct.pack("!I", val & 0xFFFFFFFF)
        self.entries.append((tag, self.RPM_INT32, data, 1))

    def add_int32_array(self, tag: int, vals: List[int]):
        data = struct.pack(f"!{len(vals)}I", *(v & 0xFFFFFFFF for v in vals))
        self.entries.append((tag, self.RPM_INT32, data, len(vals)))

    def add_bin(self, tag: int, val: bytes):
        self.entries.append((tag, self.RPM_BIN, val, len(val)))

    def build(self) -> bytes:
        # RPM tags in header index must be sorted in strictly ascending order
        self.entries.sort(key=lambda e: e[0])

        nindex = len(self.entries)
        index_bytes = bytearray()
        data_bytes = bytearray()

        for tag, typ, raw_val, count in self.entries:
            # Alignment rules for data store
            if typ in (self.RPM_INT16,):
                pad = (2 - (len(data_bytes) % 2)) % 2
            elif typ in (self.RPM_INT32,):
                pad = (4 - (len(data_bytes) % 4)) % 4
            elif typ in (self.RPM_INT64,):
                pad = (8 - (len(data_bytes) % 8)) % 8
            else:
                pad = 0

            data_bytes.extend(b"\x00" * pad)
            offset = len(data_bytes)
            data_bytes.extend(raw_val)

            # 16-byte index entry: tag(4), type(4), offset(4), count(4)
            index_bytes.extend(struct.pack("!IIII", tag, typ, offset, count))

        hsize = len(data_bytes)
        # Header magic: \x8e\xad\xe8\x01 + 4 bytes reserved
        header_prefix = b"\x8e\xad\xe8\x01\x00\x00\x00\x00" + struct.pack("!II", nindex, hsize)
        return header_prefix + bytes(index_bytes) + bytes(data_bytes)


def build_rpm(
    name: str,
    version: str,
    release: str,
    arch: str,
    summary: str,
    description: str,
    vendor: str,
    license_name: str,
    url: str,
    files: List[Dict[str, Any]],
    postin_script: str = "",
) -> bytes:
    """Builds a complete binary RPM package byte stream."""
    now = int(time.time())
    rpm_arch = "aarch64" if arch in ("arm64", "aarch64") else "x86_64"

    # 1. Build CPIO archive for payload
    cpio_buf = bytearray()
    ino = 1
    total_uncompressed_size = 0

    # Sort files by path for deterministic packaging
    files_sorted = sorted(files, key=lambda f: f["dest_path"])

    for f in files_sorted:
        dest_path = f["dest_path"]
        content = f["content"]
        mode = f.get("mode", 0o100644)
        mtime = f.get("mtime", now)
        total_uncompressed_size += len(content)

        cpio_buf.extend(make_cpio_entry(dest_path, content, mode, mtime, ino))
        ino += 1

    cpio_buf.extend(make_cpio_trailer())
    uncompressed_payload_size = len(cpio_buf)

    # 2. Compress payload with gzip
    compressed_payload = gzip.compress(bytes(cpio_buf), compresslevel=9)

    # 3. Build General Header (metadata & file manifest)
    gh = RpmHeaderBuilder()

    gh.add_string(1000, name)                       # NAME
    gh.add_string(1001, version)                    # VERSION
    gh.add_string(1002, release)                    # RELEASE
    gh.add_i18n_string(1004, summary)               # SUMMARY
    gh.add_i18n_string(1005, description)           # DESCRIPTION
    gh.add_int32(1006, now)                         # BUILDTIME
    gh.add_string(1007, "prod.codes")               # BUILDHOST
    gh.add_int32(1009, total_uncompressed_size)     # SIZE
    gh.add_string(1010, "prod-code")                # DISTRIBUTION
    gh.add_string(1011, vendor)                     # VENDOR
    gh.add_string(1014, license_name)               # LICENSE
    gh.add_string(1015, vendor)                     # PACKAGER
    gh.add_i18n_string(1016, "Development/Tools")   # GROUP
    gh.add_string(1020, url)                        # URL
    gh.add_string(1021, "linux")                    # OS
    gh.add_string(1022, rpm_arch)                   # ARCH

    if postin_script:
        gh.add_string(1024, postin_script)          # POSTIN
        gh.add_string_array(1086, ["/bin/sh"])     # POSTINPROG

    # Split files into dirnames, basenames, dirindexes
    dir_to_idx = {}
    dirnames = []
    basenames = []
    dirindexes = []
    file_sizes = []
    file_modes = []
    file_mtimes = []
    file_md5s = []
    file_linktos = []
    file_flags = []
    file_users = []
    file_groups = []
    file_devices = []
    file_inodes = []
    file_langs = []

    for i, f in enumerate(files_sorted):
        full_path = "/" + f["dest_path"].lstrip("./")
        dirname, basename = os.path.split(full_path)
        dirname = dirname + "/"
        if dirname not in dir_to_idx:
            dir_to_idx[dirname] = len(dirnames)
            dirnames.append(dirname)

        dirindexes.append(dir_to_idx[dirname])
        basenames.append(basename)
        file_sizes.append(len(f["content"]))
        file_modes.append(f.get("mode", 0o100644))
        file_mtimes.append(f.get("mtime", now))
        file_md5s.append(hashlib.md5(f["content"]).hexdigest())
        file_linktos.append("")
        file_flags.append(0)
        file_users.append("root")
        file_groups.append("root")
        file_devices.append(1)
        file_inodes.append(i + 1)
        file_langs.append("")

    gh.add_int32_array(1028, file_sizes)            # FILESIZES
    gh.add_int16_array(1030, file_modes)            # FILEMODES
    gh.add_int32_array(1034, file_mtimes)           # FILEMTIMES
    gh.add_string_array(1035, file_md5s)            # FILEMD5S
    gh.add_string_array(1036, file_linktos)         # FILELINKTOS
    gh.add_int32_array(1037, file_flags)            # FILEFLAGS
    gh.add_string_array(1039, file_users)           # FILEUSERNAME
    gh.add_string_array(1040, file_groups)          # FILEGROUPNAME

    # Dependencies & capabilities
    gh.add_string_array(1047, [name])               # PROVIDES
    gh.add_int32_array(1048, [0x1000, 0x1000])      # REQUIREFLAGS (RPMSENSE_RPMLIB)
    gh.add_string_array(1049, [
        "rpmlib(CompressedFileNames)",
        "rpmlib(PayloadFilesHavePrefix)",
    ])                                              # REQUIRENAME
    gh.add_string_array(1050, ["3.0.4-1", "4.0-1"]) # REQUIREVERSION

    gh.add_int32_array(1095, file_devices)          # FILEDEVICES
    gh.add_int32_array(1096, file_inodes)           # FILEINODES
    gh.add_string_array(1097, file_langs)           # FILELANGS

    gh.add_int32_array(1112, [8])                   # PROVIDEFLAGS (RPMSENSE_EQUAL)
    gh.add_string_array(1113, [f"{version}-{release}"]) # PROVIDEVERSION

    gh.add_int32_array(1116, dirindexes)            # DIRINDEXES
    gh.add_string_array(1117, basenames)            # BASENAMES
    gh.add_string_array(1118, dirnames)             # DIRNAMES

    gh.add_string(1124, "cpio")                     # PAYLOADFORMAT
    gh.add_string(1125, "gzip")                     # PAYLOADCOMPRESSOR
    gh.add_string(1126, "9")                        # PAYLOADFLAGS

    general_header_bytes = gh.build()

    # 4. Build Signature Header
    # Signature header signs (General Header + Compressed Payload)
    header_and_payload = general_header_bytes + compressed_payload

    sig = RpmHeaderBuilder()
    sig.add_int32(1000, len(header_and_payload))                        # RPMSIGTAG_SIZE
    sig.add_bin(1004, hashlib.md5(header_and_payload).digest())         # RPMSIGTAG_MD5
    sig.add_int32(1007, uncompressed_payload_size)                      # RPMSIGTAG_PAYLOADSIZE
    sig.add_string(1010, hashlib.sha1(general_header_bytes).hexdigest()) # RPMSIGTAG_SHA1

    sig_header_bytes = sig.build()

    # Signature header must be padded to 8-byte boundary
    sig_pad = (8 - (len(sig_header_bytes) % 8)) % 8
    sig_header_bytes_padded = sig_header_bytes + (b"\x00" * sig_pad)

    # 5. Lead (96 bytes)
    lead = bytearray(96)
    lead[0:4] = b"\xed\xab\xee\xdb"         # magic
    lead[4] = 3                             # major version (RPM v3/v4 binary lead)
    lead[5] = 0                             # minor version
    lead[6:8] = b"\x00\x00"                 # type: 0 (binary)
    lead[8:10] = b"\x00\x01"                # arch: 1
    pkg_name_bytes = f"{name}-{version}-{release}".encode("utf-8")[:65]
    lead[10:10 + len(pkg_name_bytes)] = pkg_name_bytes
    lead[76:78] = b"\x00\x01"               # os: 1 (Linux)
    lead[78:80] = b"\x00\x05"               # sigtype: 5 (Header-style)

    # Complete RPM stream: Lead + Padded Sig Header + General Header + Compressed Payload
    return bytes(lead) + sig_header_bytes_padded + general_header_bytes + compressed_payload


def main():
    parser = argparse.ArgumentParser(description="Package prod-code into an RPM package (.rpm)")
    parser.add_argument("--version", required=True, help="Package version (e.g. 0.3.23)")
    parser.add_argument("--release", default="1", help="RPM release number (default: 1)")
    parser.add_argument("--arch", default="x86_64", help="Architecture (x86_64, aarch64, arm64, amd64)")
    parser.add_argument("--client-bin", required=True, help="Path to prod-code binary")
    parser.add_argument("--service-file", required=True, help="Path to prod-code-gateway.service")
    parser.add_argument("--readme", required=True, help="Path to README.md")
    parser.add_argument("--output", required=True, help="Output .rpm file path")
    args = parser.parse_args()

    if not os.path.exists(args.client_bin):
        sys.exit(f"Error: client binary {args.client_bin} not found")
    if not os.path.exists(args.service_file):
        sys.exit(f"Error: service file {args.service_file} not found")
    if not os.path.exists(args.readme):
        sys.exit(f"Error: readme file {args.readme} not found")

    with open(args.client_bin, "rb") as f:
        bin_content = f.read()
    with open(args.service_file, "rb") as f:
        svc_content = f.read()
    with open(args.readme, "rb") as f:
        readme_content = f.read()

    files = [
        {
            "dest_path": "./usr/bin/prod-code",
            "content": bin_content,
            "mode": 0o100755,
        },
        {
            "dest_path": "./usr/lib/systemd/user/prod-code-gateway.service",
            "content": svc_content,
            "mode": 0o100644,
        },
        {
            "dest_path": "./usr/share/doc/prod-code/README.md",
            "content": readme_content,
            "mode": 0o100644,
        },
    ]

    postin_script = (
        "#!/bin/sh\n"
        "if command -v systemctl >/dev/null 2>&1; then\n"
        "    systemctl daemon-reload || true\n"
        "fi\n"
        "echo 'prod-code installed successfully. Run \"prod-code status\" or \"prod-code --help\".'\n"
        "exit 0\n"
    )

    rpm_bytes = build_rpm(
        name="prod-code",
        version=args.version.lstrip("v"),
        release=args.release,
        arch=args.arch,
        summary="Remote Code Intelligence for AI coding agents and editors",
        description=(
            "prod-code mirrors your repository to cluster nodes with warm language\n"
            "servers (rust-analyzer, gopls, clangd, basedpyright, typescript-language-server,\n"
            "sourcekit-lsp) and runs builds, tests, refactorings and MCP tools remotely."
        ),
        vendor="Alexander Panasenko <alex@prod.codes>",
        license_name="Apache-2.0",
        url="https://prod.codes",
        files=files,
        postin_script=postin_script,
    )

    os.makedirs(os.path.dirname(os.path.abspath(args.output)), exist_ok=True)
    with open(args.output, "wb") as f:
        f.write(rpm_bytes)

    print(f"✓ RPM package built: {args.output} ({len(rpm_bytes):,} bytes)")


if __name__ == "__main__":
    main()
