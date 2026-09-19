//! Workspace file scanner for fast sync over 10G LAN.

use anyhow::Result;
use prod_code_protocol::FileDelta;
use std::path::Path;

const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024; // 20 MiB per file limit

/// Scan workspace directory and generate FileDelta list, filtering out build artifacts and VCS.
pub fn scan_workspace_files(root: &Path, subpath: Option<&Path>) -> Result<Vec<FileDelta>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let target_dir = match subpath {
        Some(sub) => {
            if sub.is_absolute() {
                std::fs::canonicalize(sub).unwrap_or_else(|_| sub.to_path_buf())
            } else {
                let joined = canonical_root.join(sub);
                std::fs::canonicalize(&joined).unwrap_or(joined)
            }
        }
        None => canonical_root.clone(),
    };

    if !target_dir.exists() {
        anyhow::bail!("Path {:?} does not exist", target_dir);
    }

    let mut deltas = Vec::new();

    if target_dir.is_file() {
        let rel_path = target_dir
            .strip_prefix(&canonical_root)
            .unwrap_or(&target_dir)
            .to_string_lossy()
            .to_string();
        let content = std::fs::read(&target_dir)?;
        deltas.push(FileDelta {
            relative_path: rel_path,
            content: Some(content),
            is_executable: false,
        });
        return Ok(deltas);
    }

    walk_dir(&target_dir, &canonical_root, &mut deltas)?;
    Ok(deltas)
}

fn walk_dir(current_dir: &Path, root: &Path, deltas: &mut Vec<FileDelta>) -> Result<()> {
    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        // Ignore VCS, build directories, and transient editor files
        if name_str.starts_with('.') && name_str != ".gitignore" {
            continue;
        }
        if name_str == "target"
            || name_str == "node_modules"
            || name_str == "vendor"
            || name_str == "dist"
            || name_str == "build"
            || name_str == ".DS_Store"
        {
            continue;
        }

        if path.is_dir() {
            walk_dir(&path, root, deltas)?;
        } else if path.is_file() {
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.len() > MAX_FILE_SIZE {
                continue;
            }

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();

            if let Ok(content) = std::fs::read(&path) {
                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;

                deltas.push(FileDelta {
                    relative_path: rel_path,
                    content: Some(content),
                    is_executable,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_workspace_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/app"), "binary").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "git").unwrap();

        let deltas = scan_workspace_files(root, None).unwrap();
        let paths: Vec<_> = deltas.iter().map(|d| d.relative_path.as_str()).collect();

        assert!(paths.contains(&"src/main.rs") || paths.contains(&"src\\main.rs"));
        assert!(paths.contains(&"Cargo.toml"));
        assert!(!paths.iter().any(|p| p.contains("target")));
        assert!(!paths.iter().any(|p| p.contains(".git")));
    }
}
