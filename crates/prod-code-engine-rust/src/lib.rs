//! Rust analysis engine for prod-code, wrapping ra_ap_ide::AnalysisHost directly.

use std::path::PathBuf;

pub struct RustEngine {
    pub workspace_root: PathBuf,
}

impl RustEngine {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    pub fn is_rust_workspace(path: &std::path::Path) -> bool {
        path.join("Cargo.toml").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_detection() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!RustEngine::is_rust_workspace(temp.path()));
        std::fs::write(temp.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        assert!(RustEngine::is_rust_workspace(temp.path()));
    }
}
