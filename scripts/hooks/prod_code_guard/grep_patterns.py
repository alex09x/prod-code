# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
#
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Grep symbol classification, filename/fixture exemptions, and BRE-to-ripgrep conversion."""

import os
import re

IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
DEFINITION = re.compile(
    r"^(pub(\(crate\))?\s+)?(async\s+)?(export\s+)?"
    r"(fn|func|def|class|struct|enum|trait|interface|type|impl|protocol|extension)\s+(\([^)]*\)\s*)?"
    r"(?P<name>" + IDENT + r")"
)

RESOURCE_EXTENSIONS = {
    # Data & config formats
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "xml",
    "csv",
    "tsv",
    "sql",
    "graphql",
    "gql",
    "proto",
    # Text, documentation & logs
    "md",
    "markdown",
    "txt",
    "rst",
    "adoc",
    "pdf",
    "log",
    "diff",
    "patch",
    # Config & lockfiles
    "lock",
    "ini",
    "cfg",
    "conf",
    "env",
    "plist",
    "properties",
    "map",
    "snap",
    # Web & styles
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "svg",
    # Images & media
    "png",
    "jpg",
    "jpeg",
    "gif",
    "ico",
    "webp",
    "bmp",
    "tiff",
    # Archives & binaries
    "zip",
    "tar",
    "gz",
    "tgz",
    "bz2",
    "xz",
    "wasm",
    "bin",
    "dat",
    "data",
    "out",
    # Shell & scripts
    "sh",
    "bash",
    "zsh",
    "fish",
    "bat",
    "ps1",
    # Source code files
    "rs",
    "go",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "cjs",
    "py",
    "pyi",
    "c",
    "cc",
    "cpp",
    "cxx",
    "h",
    "hh",
    "hpp",
    "hxx",
    "swift",
    "kt",
    "kts",
    "java",
    "cs",
    "rb",
    "php",
    "scala",
    "zig",
    "lua",
    "r",
    "dart",
    "ex",
    "exs",
    "erl",
    "hs",
    "clj",
    "jl",
    "asm",
    "s",
}

SPECIAL_FILENAMES = {
    "makefile",
    "dockerfile",
    "containerfile",
    "license",
    "readme",
    "cargo.lock",
    "gemfile",
    "procfile",
    "vagrantfile",
}


def looks_like_symbol(name):
    """A code identifier rather than a word: snake_case, camelCase or PascalCase with an inner
    capital, or qualified (Type::method, pkg.Func). `error`, `TODO` and `Price` stay words.
    Filenames, extensions, and resource references (meta.json, Cargo.lock, README.md) stay words.
    Trailing-underscore prefixes (`test_`, `reserve_`) stay text patterns (#1119)."""
    if not re.fullmatch(IDENT + r"((::|\.)" + IDENT + r")*", name) or len(name) < 3:
        return False
    if name.endswith("_"):
        return False
    if "/" in name or "\\" in name:
        return False
    if name.lower() in SPECIAL_FILENAMES:
        return False
    if "::" in name:
        return True
    if "." in name:
        parts = name.split(".")
        ext = parts[-1].lower()
        if ext in RESOURCE_EXTENSIONS:
            return False
        if any(p.lower() in RESOURCE_EXTENSIONS for p in parts[1:]):
            return False
        return any(
            "_" in p.strip("_") or bool(re.search(r"[a-z][A-Z]", p)) or p[:1].isupper()
            for p in parts[1:]
        )
    if name.isupper():
        return False
    return "_" in name.strip("_") or bool(re.search(r"[a-z][A-Z]", name))


def symbol_in(pattern):
    """The symbol a grep pattern looks up, or None for a text search."""
    p = pattern.strip()
    for token in (r"\b", r"\<", r"\>", "^", "$", r"\s+", r"\s*"):
        p = p.replace(token, " " if token in (r"\s+", r"\s*") else "")
    p = re.sub(r"\s+", " ", p).strip()
    d = DEFINITION.match(p)
    if d and len(p) - d.end() <= 12:
        name = d.group("name")
        tail = p[d.end() :].strip()
        if name.endswith("_") or len(name) < 3:
            return None
        if tail and not re.match(r"^(\\\(|\(|<|\{|:|$)", tail):
            return None
        return name
    q = re.sub(r"^(\\?\.|->)", "", p)
    q = re.sub(r"(\\\(|\()$", "", q)
    return q if looks_like_symbol(q) else None


def is_filename_or_fixture_pattern(pat, cwd, fixed_strings):
    """Whether `pat` names a filename, file extension, or existing workspace fixture (#765)."""
    p = pat.strip()
    if not p or p.lower() in SPECIAL_FILENAMES:
        return True
    if ("/" in p or "\\" in p) and " " not in p and "|" not in p:
        if fixed_strings:
            return True
        base = os.path.basename(p)
        if "." in base and base.rsplit(".", 1)[-1].lower() in RESOURCE_EXTENSIONS:
            return True
        if cwd and (
            os.path.exists(os.path.join(cwd, p))
            or os.path.exists(os.path.join(cwd, "tests", "fixtures", p))
            or os.path.exists(os.path.join(cwd, "fixtures", p))
        ):
            return True
    if "." in p and " " not in p and "|" not in p and not p.startswith("."):
        ext = p.rsplit(".", 1)[-1].lower()
        if ext in RESOURCE_EXTENSIONS:
            return True
        if fixed_strings and re.fullmatch(r"[a-z0-9_-]{1,8}", ext):
            return True
    return bool(
        fixed_strings
        and cwd
        and (
            os.path.exists(os.path.join(cwd, p))
            or os.path.exists(os.path.join(cwd, "tests", "fixtures", p))
            or os.path.exists(os.path.join(cwd, "fixtures", p))
        )
    )


def bre_to_ripgrep(pattern):
    """Converts a basic regular expression into ripgrep syntax (#1581)."""
    out, i = [], 0
    while i < len(pattern):
        c = pattern[i]
        if c == "[":
            end = i + 1
            if pattern[end : end + 1] == "^":
                end += 1
            if pattern[end : end + 1] == "]":
                end += 1
            end = pattern.find("]", end)
            if end < 0:
                out.append(pattern[i:])
                break
            out.append(pattern[i : end + 1])
            i = end + 1
            continue
        if c == "\\" and i + 1 < len(pattern):
            nxt = pattern[i + 1]
            out.append(nxt if nxt in "|(){}+?" else c + nxt)
            i += 2
            continue
        out.append("\\" + c if c in "|(){}+?" else c)
        i += 1
    return "".join(out)
