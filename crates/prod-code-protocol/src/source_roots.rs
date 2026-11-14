/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Where a node keeps the sources of libraries, toolchains and SDKs: the locations a gateway
//! hands to clients besides its workspace copies. One list for the gateway, which reads a file
//! there, and for a client, which asks the node to search a directory there.

/// Toolchain and dependency caches, relative to the home directory of the gateway's user.
pub const HOME_SOURCE_ROOTS: [&str; 24] = [
    ".cargo/registry",
    ".cargo/git",
    ".rustup/toolchains",
    "go/pkg/mod",
    ".local/go",
    ".local/lib",
    ".npm-global/lib",
    ".bun/install",
    ".pnpm-store",
    ".local/share/pnpm",
    ".yarn/cache",
    ".cache/yarn",
    ".local/share/uv",
    ".cache/uv",
    ".cache/pypoetry",
    ".virtualenvs",
    ".pyenv",
    ".local/share/virtualenvs",
    ".local/pipx",
    ".conda",
    ".nvm",
    ".fnm",
    "Library/Developer",
    "prod-code-storage",
];

/// System SDK and library locations.
pub const SYSTEM_SOURCE_ROOTS: [&str; 14] = [
    "/snap",
    "/usr/include",
    "/usr/local/include",
    "/usr/local/Cellar",
    "/usr/lib",
    "/usr/local/lib",
    "/usr/local/go",
    "/usr/share",
    "/opt/homebrew",
    "/Applications/Xcode.app",
    "/Library/Developer",
    "/Library/Frameworks",
    "/System/Library/Frameworks",
    "/opt/conda",
];
