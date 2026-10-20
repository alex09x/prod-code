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

/// Directories no project's sources live in: dependencies, build output, virtual environments.
pub(crate) const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "build",
    "dist",
    ".venv",
    "venv",
    "__pycache__",
    ".build",
    "vendor",
    "Pods",
    "DerivedData",
    "test-fixtures",
];

/// The most nested projects a name search asks besides the checkout's own.
pub(crate) const MAX_NESTED_PROJECTS: usize = 6;

/// How far below a directory a search of its sources looks.
pub(crate) const MAX_SEARCH_DEPTH: usize = 10;

/// The most source files a name search reads to find the nested projects that mention it.
pub(crate) const MAX_SCANNED_FILES: usize = 5000;

/// Returns `true` when `path` sits inside a nested git repository or git worktree under `root`
/// (a subdirectory below `root` that has its own `.git` file or directory, #1071).
pub(crate) fn is_in_nested_git_worktree(root: &Path, path: &Path) -> bool {
    let resolved = if let Ok(rel) = path.strip_prefix(root) {
        root.join(rel)
    } else if let Some(rel) =
        crate::remote_fs::workspace_relative_path(root, &path.to_string_lossy())
    {
        root.join(rel)
    } else {
        let canon_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let canon_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if canon_path.starts_with(&canon_root) {
            return is_in_nested_git_worktree(&canon_root, &canon_path);
        }
        return false;
    };
    let Ok(rel) = resolved.strip_prefix(root) else {
        return false;
    };
    let mut cur = root.to_path_buf();
    let mut comps = rel.components().peekable();
    while let Some(comp) = comps.next() {
        cur.push(comp.as_os_str());
        if (comps.peek().is_some() || cur.is_dir()) && cur.join(".git").symlink_metadata().is_ok() {
            return true;
        }
    }
    false
}

/// The files under `dir` a search of the checkout looks at, in path order: what git does not
/// ignore, hidden directories, [`SKIPPED_DIRS`], and nested git worktrees/subrepositories left
/// out, at most [`MAX_SEARCH_DEPTH`] levels down (#358, #1071).
pub(crate) fn source_files(dir: &Path) -> impl Iterator<Item = std::path::PathBuf> {
    ignore::WalkBuilder::new(dir)
        .max_depth(Some(MAX_SEARCH_DEPTH))
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|t| t.is_dir())
                || (!SKIPPED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
                    && entry.path().join(".git").symlink_metadata().is_err())
        })
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
}

/// A source file of the project at `dir` in its main language (shortest path under `src`
/// first), used to make an LSP server load the project before a workspace-level query.
pub(crate) fn representative_source_file(dir: &Path) -> Option<std::path::PathBuf> {
    let project_root = dir
        .ancestors()
        .find(|ancestor| crate::sync::manifest_engine_at(ancestor).is_some())
        .or_else(|| {
            dir.ancestors()
                .find(|ancestor| crate::sync::expected_engine(ancestor).is_some())
        })
        .unwrap_or(dir);
    let (dir_subpath, language) = crate::sync::engine_project(project_root, dir);
    let exts: &[&str] = match language? {
        "rust" => &["rs"],
        "go" => &["go"],
        "cpp" => &["cpp", "cc", "cxx", "c", "hpp", "h"],
        "python" => &["py"],
        "typescript" => &["ts", "tsx", "mts", "js", "jsx"],
        "swift" => &["swift"],
        _ => return None,
    };
    let mut best: Option<(usize, usize, std::path::PathBuf)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if depth < 4
                    && !SKIPPED_DIRS.contains(&name.as_str())
                    && !name.starts_with('.')
                    && path.join(".git").symlink_metadata().is_err()
                {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !exts.contains(&ext) || name.ends_with(".d.ts") {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let outside_src = usize::from(!(rel.starts_with("src/") || rel.starts_with("lib/")));
            let is_test = usize::from(rel.contains("test") || rel.contains("spec"));
            let key = (outside_src + is_test, rel.len());
            // A file of a nested project that is its own (another language, or a crate the
            // root workspace leaves out) would open the session in that project (#335).
            if best.as_ref().is_none_or(|(a, b, _)| key < (*a, *b))
                && crate::sync::engine_project(project_root, &path).0 == dir_subpath
            {
                best = Some((key.0, key.1, path));
            }
        }
    }
    best.map(|(_, _, p)| p)
}

/// Maximum bytes read from any one source while searching for names synchronously.
pub(crate) const MAX_NAME_SCAN_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Whether `text` has `name` as a whole word.
pub(crate) fn names_word(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.split(|c: char| !is_word(c))
        .any(|word| word.eq_ignore_ascii_case(name))
}

/// Reads only the prefix needed for bounded symbol-name discovery. Large generated sources do
/// not get to exceed the overall search budget through one unbounded synchronous read.
pub(crate) fn read_name_scan_text(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_NAME_SCAN_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// One source file of each project in the checkout besides the root's, with the project's
/// directory (relative) and engine: a nested project of another language, or a loose file of
/// one, as `engine_project` places them (#247, #318).
pub(crate) fn nested_project_anchors(
    root: &Path,
    deadline: tokio::time::Instant,
) -> Vec<(std::path::PathBuf, String, &'static str)> {
    let mut seen_dirs = std::collections::HashSet::new();
    let mut projects = std::collections::HashSet::new();
    let mut anchors = Vec::new();
    for path in source_files(root) {
        if tokio::time::Instant::now() >= deadline {
            return anchors;
        }
        let Some(engine) = crate::sync::engine_for_file(&path) else {
            continue;
        };
        // One look per directory and language: its files belong to the same project.
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if !seen_dirs.insert((dir, engine)) {
            continue;
        }
        if let (Some(subpath), Some(engine)) = crate::sync::engine_project(root, &path)
            && projects.insert((subpath.clone(), engine))
        {
            anchors.push((path, subpath, engine));
            if anchors.len() >= MAX_NESTED_PROJECTS {
                return anchors;
            }
        }
    }
    anchors
}

/// One file of each nested project whose sources name `name`, in path order: the projects to
/// ask first (#358). A walk that takes the first projects it meets spent every slot on a C++
/// dependency and loose scripts while the declaration sat in a Swift package after them.
pub(crate) fn projects_naming(
    root: &Path,
    name: &str,
    deadline: tokio::time::Instant,
) -> Vec<(std::path::PathBuf, String, &'static str)> {
    let root_engine = crate::sync::expected_engine(root);
    let mut projects = std::collections::HashSet::new();
    let mut anchors = Vec::new();
    for path in source_files(root)
        .filter(|path| {
            crate::sync::engine_for_file(path).is_some_and(|e| {
                Some(e) != root_engine || crate::sync::engine_project(root, path).0.is_some()
            })
        })
        .take(MAX_SCANNED_FILES)
    {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if !read_name_scan_text(&path).is_some_and(|text| names_word(&text, name)) {
            continue;
        }
        if let (Some(subpath), Some(engine)) = crate::sync::engine_project(root, &path)
            && projects.insert((subpath.clone(), engine))
        {
            anchors.push((path, subpath, engine));
            if anchors.len() >= MAX_NESTED_PROJECTS {
                break;
            }
        }
    }
    anchors
}
