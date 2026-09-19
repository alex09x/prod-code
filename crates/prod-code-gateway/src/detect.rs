//! Multi-language workspace detection.
//!
//! Automatically identifies the language engine suited for a given workspace
//! based on project manifests (Cargo.toml, go.mod, package.json, pyproject.toml, etc.).

use prod_code_protocol::messages::EngineKind;
use std::path::Path;
use std::str::FromStr;

/// Manifest markers used to identify project language types.
const RUST_MARKERS: &[&str] = &["Cargo.toml"];
const GO_MARKERS: &[&str] = &["go.mod", "go.work"];
const PYTHON_MARKERS: &[&str] = &[
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "setup.cfg",
    "Pipfile",
];
const CPP_MARKERS: &[&str] = &[
    "compile_commands.json",
    "CMakeLists.txt",
    "meson.build",
    ".clangd",
];
const SWIFT_MARKERS: &[&str] = &["Package.swift"];
const TYPESCRIPT_MARKERS: &[&str] = &[
    "tsconfig.json",
    "package.json",
    "jsconfig.json",
    "deno.json",
    "deno.jsonc",
];

/// A Swift package manifest or an Xcode project/workspace bundle at the root.
pub fn has_swift_project(root: &Path) -> bool {
    if SWIFT_MARKERS.iter().any(|m| root.join(m).exists()) {
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

/// Detect the primary engine kind for the specified workspace path.
///
/// Priority order:
/// 1. Rust (`Cargo.toml`)
/// 2. Go (`go.mod`, `go.work`)
/// 3. Python (`pyproject.toml`, `requirements.txt`, `setup.py`, etc.)
/// 4. TypeScript / JavaScript (`tsconfig.json`, `package.json`, etc.)
/// 5. Generic LSP fallback
pub fn detect_engine(root: &Path) -> EngineKind {
    for marker in RUST_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Rust;
        }
    }
    for marker in GO_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Go;
        }
    }
    if has_swift_project(root) {
        return EngineKind::Swift;
    }
    for marker in CPP_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Cpp;
        }
    }
    for marker in PYTHON_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Python;
        }
    }
    for marker in TYPESCRIPT_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::TypeScript;
        }
    }
    EngineKind::Generic
}

/// Detect all applicable engine kinds for a workspace (e.g. polyglot monorepos).
pub fn detect_all_engines(root: &Path) -> Vec<EngineKind> {
    let mut engines = Vec::new();

    if RUST_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Rust);
    }
    if GO_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Go);
    }
    if PYTHON_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Python);
    }
    if TYPESCRIPT_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::TypeScript);
    }
    if CPP_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Cpp);
    }
    if has_swift_project(root) {
        engines.push(EngineKind::Swift);
    }

    if engines.is_empty() {
        engines.push(EngineKind::Generic);
    }

    engines
}

/// Resolve the effective engine, honoring an explicit client preference if provided.
pub fn resolve_engine(root: &Path, preferred: Option<&str>) -> EngineKind {
    if let Some(pref) = preferred.filter(|p| !p.trim().is_empty()) {
        return EngineKind::from_str(pref.trim()).unwrap_or(EngineKind::Generic);
    }
    detect_engine(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_detect_cpp_and_swift_workspaces() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("CMakeLists.txt"), "project(x)").unwrap();
        assert_eq!(detect_engine(dir.path()), EngineKind::Cpp);
        std::fs::write(
            dir.path().join("Package.swift"),
            "// swift-tools-version:5.9",
        )
        .unwrap();
        assert_eq!(
            detect_engine(dir.path()),
            EngineKind::Swift,
            "Swift outranks C++"
        );
        let xc = tempdir().unwrap();
        std::fs::create_dir_all(xc.path().join("App.xcodeproj")).unwrap();
        assert_eq!(detect_engine(xc.path()), EngineKind::Swift);
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(
            detect_engine(dir.path()),
            EngineKind::Rust,
            "Rust outranks Swift"
        );
        assert!(detect_all_engines(dir.path()).contains(&EngineKind::Cpp));
    }

    #[test]
    fn test_detect_rust_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        assert_eq!(detect_engine(dir.path()), EngineKind::Rust);
        assert_eq!(detect_all_engines(dir.path()), vec![EngineKind::Rust]);
    }

    #[test]
    fn test_detect_go_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("go.mod"),
            "module example.com/test\n\ngo 1.22",
        )
        .unwrap();
        assert_eq!(detect_engine(dir.path()), EngineKind::Go);
        assert_eq!(detect_all_engines(dir.path()), vec![EngineKind::Go]);
    }

    #[test]
    fn test_detect_python_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"test\"",
        )
        .unwrap();
        assert_eq!(detect_engine(dir.path()), EngineKind::Python);

        let dir2 = tempdir().unwrap();
        std::fs::write(dir2.path().join("requirements.txt"), "requests>=2.0").unwrap();
        assert_eq!(detect_engine(dir2.path()), EngineKind::Python);
    }

    #[test]
    fn test_detect_typescript_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{\"name\": \"test\"}").unwrap();
        assert_eq!(detect_engine(dir.path()), EngineKind::TypeScript);

        let dir2 = tempdir().unwrap();
        std::fs::write(dir2.path().join("tsconfig.json"), "{}").unwrap();
        assert_eq!(detect_engine(dir2.path()), EngineKind::TypeScript);
    }

    #[test]
    fn test_detect_empty_fallback() {
        let dir = tempdir().unwrap();
        assert_eq!(detect_engine(dir.path()), EngineKind::Generic);
        assert_eq!(detect_all_engines(dir.path()), vec![EngineKind::Generic]);
    }

    #[test]
    fn test_polyglot_monorepo() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("go.mod"), "module test").unwrap();

        // Primary follows priority (Rust > Go > Python > TS)
        assert_eq!(detect_engine(dir.path()), EngineKind::Rust);

        // All detected engines returns all 3
        let all = detect_all_engines(dir.path());
        assert_eq!(
            all,
            vec![EngineKind::Rust, EngineKind::Go, EngineKind::TypeScript]
        );
    }

    #[test]
    fn test_preferred_engine_override() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]").unwrap();

        // With no preference, detects Rust
        assert_eq!(resolve_engine(dir.path(), None), EngineKind::Rust);

        // With preference for Go, overrides to Go
        assert_eq!(resolve_engine(dir.path(), Some("go")), EngineKind::Go);
        assert_eq!(
            resolve_engine(dir.path(), Some("python")),
            EngineKind::Python
        );
    }
}
