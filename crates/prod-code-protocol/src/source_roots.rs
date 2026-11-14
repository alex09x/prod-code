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

/// Whether `path` is written like a source location of a node: under a system location, or
/// with the directories of a home location in it (`…/.cargo/registry/…`; which home is the
/// node's is not known here), or in a Go module cache (`…/pkg/mod/…`). The node decides what
/// it serves; this tells a dependency's source from another directory of the machine that
/// asks, such as a second checkout.
pub fn names_source_location(path: &std::path::Path) -> bool {
    if SYSTEM_SOURCE_ROOTS
        .iter()
        .any(|root| path.starts_with(root))
    {
        return true;
    }
    let parts: Vec<&std::ffi::OsStr> = path.iter().collect();
    HOME_SOURCE_ROOTS
        .iter()
        .copied()
        .chain(["pkg/mod"])
        .any(|relative| {
            let wanted: Vec<&std::ffi::OsStr> = std::path::Path::new(relative).iter().collect();
            parts.windows(wanted.len()).any(|window| window == wanted)
        })
}

#[cfg(test)]
mod tests {
    use super::names_source_location;
    use std::path::Path;

    #[test]
    fn a_dependency_cache_is_told_from_another_directory() {
        for source in [
            "/home/build/.cargo/registry/src/index.crates.io-1/demo-1.0.0/src",
            "/Users/build/.rustup/toolchains/stable/lib/rustlib/src/rust/library/core",
            "/home/build/go/pkg/mod/example.com/demo@v1.0.0",
            "/srv/gopath/pkg/mod/example.com/demo@v1.0.0",
            "/usr/include/stdio.h",
        ] {
            assert!(names_source_location(Path::new(source)), "{source}");
        }
        for other in [
            "/home/build/checkouts/second/src/lib.rs",
            "/home/build/.cargo-notes/registry.txt",
            "/usr/includes/stdio.h",
            "/tmp/pkg/models",
        ] {
            assert!(!names_source_location(Path::new(other)), "{other}");
        }
    }
}
