/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use crate::detect::markers::{
    BALLERINA_MARKERS, CLOJURE_MARKERS, ERLANG_MARKERS, HASKELL_MARKERS, JULIA_MARKERS,
    LUA_MARKERS, NIM_MARKERS, OCAML_MARKERS, PERL_MARKERS, R_MARKERS, RACKET_MARKERS,
    SHELL_MARKERS, SOLIDITY_MARKERS,
};

/// A Lua project (.luarc.json, .luacheckrc, *.rockspec) at the root.
pub fn has_lua_project(root: &Path) -> bool {
    if LUA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".rockspec")
            })
        })
        .unwrap_or(false)
}

/// A Haskell project (cabal.project, stack.yaml, package.yaml, *.cabal) at the root.
pub fn has_haskell_project(root: &Path) -> bool {
    if HASKELL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".cabal")
            })
        })
        .unwrap_or(false)
}

/// An OCaml project (dune-project, dune, *.opam) at the root.
pub fn has_ocaml_project(root: &Path) -> bool {
    if OCAML_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".opam")
            })
        })
        .unwrap_or(false)
}

/// A Clojure project (project.clj, deps.edn) at the root.
pub fn has_clojure_project(root: &Path) -> bool {
    CLOJURE_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Julia project (Project.toml, JuliaProject.toml) at the root.
pub fn has_julia_project(root: &Path) -> bool {
    if JULIA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    let proj = root.join("Project.toml");
    if proj.is_file()
        && let Ok(text) = std::fs::read_to_string(&proj)
        && (text.contains("[deps]") || text.contains("uuid ="))
    {
        return true;
    }
    false
}

/// A Shell project (.shellcheckrc, *.sh) at the root.
pub fn has_shell_project(root: &Path) -> bool {
    if SHELL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".sh")
            })
        })
        .unwrap_or(false)
}

/// An R project (DESCRIPTION, NAMESPACE) at the root.
pub fn has_r_project(root: &Path) -> bool {
    R_MARKERS.iter().any(|m| root.join(m).exists())
}

/// An Erlang project (rebar.config, rebar.lock, erlang.mk) at the root.
pub fn has_erlang_project(root: &Path) -> bool {
    ERLANG_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Perl project (cpanfile, Makefile.PL, Build.PL, dist.ini) at the root.
pub fn has_perl_project(root: &Path) -> bool {
    PERL_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Solidity project (foundry.toml, hardhat.config.*) at the root.
pub fn has_solidity_project(root: &Path) -> bool {
    SOLIDITY_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Nim project (*.nimble, nim.cfg) at the root.
pub fn has_nim_project(root: &Path) -> bool {
    if NIM_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".nimble"))
        })
        .unwrap_or(false)
}

/// A Racket project (info.rkt or *.rkt) at the root.
pub fn has_racket_project(root: &Path) -> bool {
    if RACKET_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".rkt"))
        })
        .unwrap_or(false)
}

/// A Ballerina project (Ballerina.toml, *.bal) at the root.
pub fn has_ballerina_project(root: &Path) -> bool {
    if BALLERINA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".bal"))
        })
        .unwrap_or(false)
}
