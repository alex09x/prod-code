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
    ADA_MARKERS, ASSEMBLY_MARKERS, C_SOURCE_EXTENSIONS, CSHARP_MARKERS, D_MARKERS, FORTRAN_MARKERS,
    MAKEFILES, SWIFT_MARKERS, SYSTEMVERILOG_MARKERS, V_MARKERS, VHDL_MARKERS, WAT_MARKERS,
};

/// A Makefile next to C or C++ sources, at the root or in `src/`: a C/C++ project built with
/// Make (#404). It ranks below every other manifest, since Go, Python and JavaScript
/// repositories keep a Makefile of tasks too, and a Makefile with no C sources is not C.
pub fn is_make_cpp_project(root: &Path) -> bool {
    let has_c_sources = |dir: &Path| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| C_SOURCE_EXTENSIONS.contains(&x))
            })
        })
    };
    MAKEFILES.iter().any(|m| root.join(m).is_file())
        && (has_c_sources(root) || has_c_sources(&root.join("src")))
}

/// An XcodeGen spec at the root: a `project.yml` with top-level `targets:`, from which the
/// Xcode project, usually not committed, is generated (#404).
pub fn is_xcodegen_spec(root: &Path) -> bool {
    std::fs::read_to_string(root.join("project.yml"))
        .is_ok_and(|text| text.lines().any(|line| line.starts_with("targets:")))
}

/// A Swift package manifest, an XcodeGen spec, or an Xcode project/workspace bundle at the root.
pub fn has_swift_project(root: &Path) -> bool {
    if SWIFT_MARKERS.iter().any(|m| root.join(m).exists()) || is_xcodegen_spec(root) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace")
            })
        })
        .unwrap_or(false)
}

/// A C# project (.csproj, .sln) or global.json configuration at the root.
pub fn has_csharp_project(root: &Path) -> bool {
    if CSHARP_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".csproj") || name.ends_with(".sln")
            })
        })
        .unwrap_or(false)
}

/// An F# project (*.fsproj) at the root.
pub fn has_fsharp_project(root: &Path) -> bool {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".fsproj"))
        })
        .unwrap_or(false)
}

/// A D project (dub.json, dub.sdl) at the root.
pub fn has_d_project(root: &Path) -> bool {
    D_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Fortran project (fpm.toml) at the root.
pub fn has_fortran_project(root: &Path) -> bool {
    FORTRAN_MARKERS.iter().any(|m| root.join(m).exists())
}

/// An Ada project (*.gpr, *.adb, *.ads) at the root.
pub fn has_ada_project(root: &Path) -> bool {
    if ADA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".gpr") || name.ends_with(".adb") || name.ends_with(".ads")
            })
        })
        .unwrap_or(false)
}

/// A V project (v.mod or *.v) at the root.
pub fn has_v_project(root: &Path) -> bool {
    if V_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".vsh")
            })
        })
        .unwrap_or(false)
}

/// An Assembly project (*.s, *.asm, *.S) at the root.
pub fn has_assembly_project(root: &Path) -> bool {
    if ASSEMBLY_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".s") || name.ends_with(".asm") || name.ends_with(".S")
            })
        })
        .unwrap_or(false)
}

/// A WebAssembly text format project (wat.json, *.wat, *.wast) at the root.
pub fn has_wat_project(root: &Path) -> bool {
    if WAT_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".wat") || name.ends_with(".wast")
            })
        })
        .unwrap_or(false)
}

/// A SystemVerilog / Verilog project (verilator.f, *.sv, *.svh) at the root.
pub fn has_systemverilog_project(root: &Path) -> bool {
    if SYSTEMVERILOG_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".sv") || name.ends_with(".svh")
            })
        })
        .unwrap_or(false)
}

/// A VHDL project (vunit.py, *.vhd, *.vhdl) at the root.
pub fn has_vhdl_project(root: &Path) -> bool {
    if VHDL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".vhd") || name.ends_with(".vhdl")
            })
        })
        .unwrap_or(false)
}
