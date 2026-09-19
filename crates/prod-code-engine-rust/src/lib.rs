//! High-performance, in-memory Rust analysis engine for prod-code directly utilizing `ra_ap_ide::AnalysisHost`.

use anyhow::{Context, Result};
use ra_ap_ide::{
    AnalysisHost, FileId, FilePosition, FileRange, FileStructureConfig, FindAllRefsConfig,
    GotoDefinitionConfig, HoverConfig, HoverDocFormat, RaFixtureConfig, TextRange, TextSize,
};
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_load_cargo::{
    LoadCargoConfig, ProcMacroServerChoice, ProjectFolders, SourceRootConfig, load_workspace_at,
};
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{CargoConfig, ProjectManifest, ProjectWorkspace};
use ra_ap_vfs::{Vfs, VfsPath};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 24-bit mask limit (0x007F_FFFF) to ensure EditionedFileId edition bits are never corrupted.
pub const MAX_SAFE_FILE_ID: u32 = 0x007F_FFFF;

pub fn is_safe_file_id(file_id: FileId) -> bool {
    file_id.index() <= MAX_SAFE_FILE_ID
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefinitionTarget {
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReferenceTarget {
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolTarget {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub detail: Option<String>,
}

/// Canonical path normalizer for VFS keys: removes `.` and `..` lexically, resolves absolute path.
pub fn normalize_vfs_path(path: &Path, workspace_root: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };

    let mut components = Vec::new();
    for comp in abs.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                components.pop();
            }
            c => components.push(c),
        }
    }
    components.into_iter().collect()
}

/// Thread-safe, multi-core analysis snapshot backed by warm Salsa database.
///
/// Snapshots can be moved across threads (`tokio::task::spawn_blocking`),
/// allowing concurrent read queries (hover, definition, references, symbols)
/// to execute simultaneously across all available CPU cores without blocking.
pub struct RustEngineSnapshot {
    pub workspace_root: PathBuf,
    analysis: ra_ap_ide::Analysis,
    vfs: Arc<std::sync::RwLock<Vfs>>,
}

impl RustEngineSnapshot {
    /// Lookup Vfs FileId for a filesystem path with safe 24-bit EditionedFileId validation.
    pub fn file_id_for_path(&self, path: &Path) -> Option<FileId> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let vfs_path = VfsPath::new_real_path(norm.to_string_lossy().to_string());
        let guard = self.vfs.read().ok()?;
        let (file_id, _) = guard.file_id(&vfs_path)?;
        if is_safe_file_id(file_id) {
            Some(file_id)
        } else {
            tracing::error!(?file_id, "FileId exceeded MAX_SAFE_FILE_ID (0x007F_FFFF)");
            None
        }
    }

    /// Lookup filesystem path for a Vfs FileId.
    pub fn path_for_file_id(&self, file_id: FileId) -> Option<PathBuf> {
        let guard = self.vfs.read().ok()?;
        let vfs_path = guard.file_path(file_id);
        vfs_path.as_path().map(|p| PathBuf::from(p.as_str()))
    }

    /// Retrieve symbol type, docs, and signature at (line, col).
    pub fn hover(&self, path: &Path, line: u32, col: u32) -> Result<Option<String>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;

        let text = self.analysis.file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));

        let file_range = FileRange {
            file_id,
            range: TextRange::empty(offset),
        };

        let config = HoverConfig {
            links_in_hover: true,
            memory_layout: None,
            documentation: true,
            keywords: true,
            format: HoverDocFormat::Markdown,
            max_trait_assoc_items_count: None,
            max_fields_count: Some(10),
            max_enum_variants_count: Some(10),
            max_subst_ty_len: ra_ap_ide::SubstTyLen::Unlimited,
            show_drop_glue: true,
            ra_fixture: RaFixtureConfig::default(),
        };

        let res = self.analysis.hover(&config, file_range)?;
        Ok(res.map(|h| h.info.markup.to_string()))
    }

    /// Jump to symbol definition from (line, col).
    pub fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;

        let text = self.analysis.file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));

        let file_pos = FilePosition { file_id, offset };
        let config = GotoDefinitionConfig {
            ra_fixture: RaFixtureConfig::default(),
        };

        let targets = match self.analysis.goto_definition(file_pos, &config)? {
            Some(range_info) => range_info.info,
            None => return Ok(vec![]),
        };

        let mut results = Vec::new();
        for target in targets {
            if let Some(target_path) = self.path_for_file_id(target.file_id) {
                let target_text = self.analysis.file_text(target.file_id)?;
                let focus = target.focus_range.unwrap_or(target.full_range);
                let (target_line, target_col) = offset_to_line_col(&target_text, focus.start());
                results.push(DefinitionTarget {
                    path: target_path,
                    line: target_line,
                    col: target_col,
                    name: target.name.to_string(),
                });
            }
        }

        Ok(results)
    }

    /// Find all references to symbol at (line, col) across entire workspace.
    pub fn find_all_refs(&self, path: &Path, line: u32, col: u32) -> Result<Vec<ReferenceTarget>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;

        let text = self.analysis.file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));

        let file_pos = FilePosition { file_id, offset };
        let config = FindAllRefsConfig {
            search_scope: None,
            ra_fixture: RaFixtureConfig::default(),
            exclude_imports: false,
            exclude_tests: false,
        };

        let search_res = match self.analysis.find_all_refs(file_pos, &config)? {
            Some(res) => res,
            None => return Ok(vec![]),
        };

        let mut results = Vec::new();
        for res in search_res {
            for (ref_file_id, refs) in res.references {
                if let (Some(ref_path), Ok(ref_text)) = (
                    self.path_for_file_id(ref_file_id),
                    self.analysis.file_text(ref_file_id),
                ) {
                    for (range, _) in refs {
                        let (ref_line, ref_col) = offset_to_line_col(&ref_text, range.start());
                        results.push(ReferenceTarget {
                            path: ref_path.clone(),
                            line: ref_line,
                            col: ref_col,
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    /// Generate outline / document symbols for a file.
    pub fn document_symbols(&self, path: &Path) -> Result<Vec<SymbolTarget>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;

        let text = self.analysis.file_text(file_id)?;
        let config = FileStructureConfig {
            exclude_locals: false,
        };
        let nodes = self.analysis.file_structure(&config, file_id)?;

        let mut results = Vec::new();
        for node in nodes {
            let (sym_line, _) = offset_to_line_col(&text, node.node_range.start());
            results.push(SymbolTarget {
                name: node.label,
                kind: format!("{:?}", node.kind),
                line: sym_line,
                detail: node.detail,
            });
        }

        Ok(results)
    }
}

/// In-memory Rust analysis engine core backed by a warm Salsa database.
pub struct RustEngine {
    pub workspace_root: PathBuf,
    host: AnalysisHost,
    vfs: Arc<std::sync::RwLock<Vfs>>,
    source_root_config: Arc<SourceRootConfig>,
}

impl RustEngine {
    /// Detect whether a directory contains a Rust workspace manifest (Cargo.toml).
    pub fn is_rust_workspace(path: &Path) -> bool {
        path.join("Cargo.toml").exists()
    }

    /// Load and index a Cargo workspace directly into in-memory Salsa DB using multi-core worker threads.
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let cargo_config = CargoConfig::default();
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: false,
            num_worker_threads: num_threads,
            proc_macro_processes: num_threads.min(8),
        };

        tracing::info!(
            ?workspace_root,
            threads = num_threads,
            "Loading Cargo workspace into Salsa database using all available CPU cores"
        );
        let (db, vfs, _proc_macro) =
            load_workspace_at(workspace_root, &cargo_config, &load_config, &|_| {})
                .map_err(|e| anyhow::anyhow!("Failed to load cargo workspace: {e}"))?;

        let abs_root = if workspace_root.is_absolute() {
            AbsPathBuf::assert_utf8(workspace_root.to_path_buf())
        } else {
            AbsPathBuf::assert_utf8(std::env::current_dir()?.join(workspace_root))
        };
        let manifest = ProjectManifest::discover_single(&abs_root)
            .map_err(|e| anyhow::anyhow!("Manifest discovery failed: {e}"))?;
        let ws = ProjectWorkspace::load(manifest, &cargo_config, &|_| {})
            .map_err(|e| anyhow::anyhow!("Project workspace load failed: {e}"))?;
        let project_folders = ProjectFolders::new(std::slice::from_ref(&ws), &[], None);
        let source_root_config = Arc::new(project_folders.source_root_config);

        let host = AnalysisHost::with_database(db);
        tracing::info!(?workspace_root, "Cargo workspace warm and ready in RAM");

        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            host,
            vfs: Arc::new(std::sync::RwLock::new(vfs)),
            source_root_config,
        })
    }

    /// Obtain a lightweight, thread-safe analysis snapshot for parallel execution.
    pub fn snapshot(&self) -> RustEngineSnapshot {
        RustEngineSnapshot {
            workspace_root: self.workspace_root.clone(),
            analysis: self.host.analysis(),
            vfs: Arc::clone(&self.vfs),
        }
    }

    /// Lookup Vfs FileId for a filesystem path.
    pub fn file_id_for_path(&self, path: &Path) -> Option<FileId> {
        self.snapshot().file_id_for_path(path)
    }

    /// Lookup filesystem path for a Vfs FileId.
    pub fn path_for_file_id(&self, file_id: FileId) -> Option<PathBuf> {
        self.snapshot().path_for_file_id(file_id)
    }

    /// Retrieve symbol type, docs, and signature at (line, col).
    pub fn hover(&self, path: &Path, line: u32, col: u32) -> Result<Option<String>> {
        self.snapshot().hover(path, line, col)
    }

    /// Jump to symbol definition from (line, col).
    pub fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        self.snapshot().goto_definition(path, line, col)
    }

    /// Find all references to symbol at (line, col) across entire workspace.
    pub fn find_all_refs(&self, path: &Path, line: u32, col: u32) -> Result<Vec<ReferenceTarget>> {
        self.snapshot().find_all_refs(path, line, col)
    }

    /// Generate outline / document symbols for a file.
    pub fn document_symbols(&self, path: &Path) -> Result<Vec<SymbolTarget>> {
        self.snapshot().document_symbols(path)
    }

    /// Single-owner fast path: Apply live buffer edits directly into Salsa DB in memory.
    pub fn apply_file_change(&mut self, path: &Path, new_text: String) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let vfs_path = VfsPath::new_real_path(norm.to_string_lossy().to_string());
        let (file_id, is_new) = if let Some(fid) = self.file_id_for_path(path) {
            (fid, false)
        } else {
            let mut vfs = self
                .vfs
                .write()
                .map_err(|e| anyhow::anyhow!("VFS lock error: {e}"))?;
            let _ = vfs.set_file_contents(vfs_path.clone(), Some(new_text.as_bytes().to_vec()));
            let fid = vfs
                .file_id(&vfs_path)
                .map(|(id, _)| id)
                .with_context(|| format!("File not found in VFS even after set: {:?}", path))?;
            (fid, true)
        };

        let mut change = ChangeWithProcMacros::default();
        if is_new {
            let vfs_guard = self
                .vfs
                .read()
                .map_err(|e| anyhow::anyhow!("VFS lock error: {e}"))?;
            let roots = self.source_root_config.partition(&vfs_guard);
            change.source_change.set_roots(roots);
        }
        change.change_file(file_id, Some(new_text));
        self.host.apply_change(change);
        Ok(())
    }
}

/// Convert 1-indexed (line, col) to 0-indexed byte offset.
fn line_col_to_offset(text: &str, target_line: u32, target_col: u32) -> Option<TextSize> {
    let mut current_line = 1;
    let mut line_start = 0;

    for (i, c) in text.char_indices() {
        if current_line == target_line {
            let line_slice = &text[line_start..];
            for (current_col, (col_offset, _)) in (1..).zip(line_slice.char_indices()) {
                if current_col == target_col {
                    return Some(TextSize::from((line_start + col_offset) as u32));
                }
            }
            return Some(TextSize::from((line_start + line_slice.len()) as u32));
        }

        if c == '\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }

    if current_line == target_line && target_col == 1 {
        return Some(TextSize::from(line_start as u32));
    }

    None
}

/// Convert 0-indexed byte offset to 1-indexed (line, col).
fn offset_to_line_col(text: &str, offset: TextSize) -> (u32, u32) {
    let target = usize::from(offset);
    let mut line = 1;
    let mut col = 1;

    for (i, c) in text.char_indices() {
        if i >= target {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }

    (line, col)
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

    #[test]
    fn test_line_col_offset_math() {
        let text = "fn main() {\n    println!(\"hello\");\n}\n";
        // Line 1 Col 1 -> 'f' (offset 0)
        assert_eq!(line_col_to_offset(text, 1, 1), Some(TextSize::from(0)));
        assert_eq!(offset_to_line_col(text, TextSize::from(0)), (1, 1));

        // Line 2 Col 5 -> 'p' (offset 16)
        let off = line_col_to_offset(text, 2, 5).unwrap();
        assert_eq!(offset_to_line_col(text, off), (2, 5));
    }

    fn create_test_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let cargo_toml = r#"[package]
name = "fixture"
version = "0.1.0"
edition = "2021"
"#;
        std::fs::write(temp.path().join("Cargo.toml"), cargo_toml).unwrap();
        let src_dir = temp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let code = r#"pub const DEFAULT_PORT: u16 = 9400;

pub struct PathTranslator {
    pub prefix: String,
}

impl PathTranslator {
    pub fn new(prefix: &str) -> Self {
        Self { prefix: prefix.to_string() }
    }
}
"#;
        let lib_path = src_dir.join("lib.rs");
        std::fs::write(&lib_path, code).unwrap();
        (temp, lib_path)
    }

    #[test]
    fn test_rust_engine_in_memory_queries() {
        let (temp, lib_path) = create_test_fixture();
        let engine =
            RustEngine::load(temp.path()).expect("Must load fixture directly into Salsa DB");

        // Test document symbols on fixture lib.rs
        let syms = engine
            .document_symbols(&lib_path)
            .expect("Must get document symbols");
        assert!(!syms.is_empty(), "Must find symbols in fixture lib.rs");
        assert!(syms.iter().any(|s| s.name == "DEFAULT_PORT"));
        assert!(syms.iter().any(|s| s.name == "PathTranslator"));

        // Test in-memory hover on DEFAULT_PORT (line 1, col 15)
        let hover = engine
            .hover(&lib_path, 1, 15)
            .expect("Must query in-memory hover");
        assert!(hover.is_some(), "Hover must resolve for DEFAULT_PORT");
        assert!(hover.unwrap().contains("DEFAULT_PORT"));

        // Test in-memory jump to definition for PathTranslator (line 3, col 15)
        let defs = engine
            .goto_definition(&lib_path, 3, 15)
            .expect("Must query definition");
        assert!(
            !defs.is_empty(),
            "Must resolve definition for PathTranslator"
        );
        assert!(defs.iter().any(|d| d.name == "PathTranslator"));
    }

    #[test]
    fn test_rust_engine_direct_mutation() {
        let (temp, lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");

        // Apply in-memory direct edit adding a new constant
        let new_code = "pub const IN_MEMORY_SUPER_FAST: u32 = 42;\n".to_string();
        engine
            .apply_file_change(&lib_path, new_code)
            .expect("Must apply direct change to Salsa DB");

        // Verify that in-memory symbols immediately reflect the new symbol without saving to disk!
        let syms = engine
            .document_symbols(&lib_path)
            .expect("Must get updated symbols");
        assert!(syms.iter().any(|s| s.name == "IN_MEMORY_SUPER_FAST"));

        // Verify hover on the new in-memory symbol!
        let hover = engine
            .hover(&lib_path, 1, 15)
            .expect("Must query hover on newly added in-memory symbol");
        assert!(hover.is_some());
        assert!(hover.unwrap().contains("IN_MEMORY_SUPER_FAST"));
    }

    #[test]
    fn test_rust_engine_add_new_untracked_file() {
        let (temp, lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");

        // Dynamically add a brand new untracked file that was never loaded initially
        let new_file_path = temp.path().join("src/helper.rs");
        let helper_code = "pub fn dynamic_helper() -> u32 { 1337 }\n".to_string();
        engine
            .apply_file_change(&new_file_path, helper_code)
            .expect("Must apply change for brand new file without panic");

        let syms = engine
            .document_symbols(&new_file_path)
            .expect("Must get document symbols for newly added file");
        assert!(syms.iter().any(|s| s.name == "dynamic_helper"));

        // Link helper module into lib.rs so Salsa builds the module tree and HIR
        let mut lib_code = std::fs::read_to_string(&lib_path).unwrap();
        lib_code.push_str("\npub mod helper;\n");
        engine
            .apply_file_change(&lib_path, lib_code)
            .expect("Must update lib.rs");

        let hover = engine
            .hover(&new_file_path, 1, 10)
            .expect("Must query hover on newly added file");
        assert!(hover.is_some());
        assert!(hover.unwrap().contains("fn dynamic_helper"));
    }

    #[test]
    fn test_snapshot_parallel_execution() {
        let (temp, lib_path) = create_test_fixture();
        let engine = RustEngine::load(temp.path()).expect("Must load fixture");

        let snap1 = engine.snapshot();
        let snap2 = engine.snapshot();
        let p1 = lib_path.clone();
        let p2 = lib_path;

        let h1 = std::thread::spawn(move || snap1.hover(&p1, 1, 15));
        let h2 = std::thread::spawn(move || snap2.document_symbols(&p2));

        let res1 = h1.join().unwrap().unwrap();
        let res2 = h2.join().unwrap().unwrap();

        assert!(res1.is_some());
        assert!(!res2.is_empty());
    }

    #[test]
    fn test_normalize_vfs_path() {
        let ws = Path::new("/workspace/project");
        assert_eq!(
            normalize_vfs_path(Path::new("src/./main.rs"), ws),
            PathBuf::from("/workspace/project/src/main.rs")
        );
        assert_eq!(
            normalize_vfs_path(Path::new("src/../Cargo.toml"), ws),
            PathBuf::from("/workspace/project/Cargo.toml")
        );
        assert_eq!(
            normalize_vfs_path(Path::new("/workspace/project/src/lib.rs"), ws),
            PathBuf::from("/workspace/project/src/lib.rs")
        );
    }

    #[test]
    fn test_safe_file_id_bounds() {
        assert_eq!(MAX_SAFE_FILE_ID, 0x007F_FFFF);
        const { assert!(MAX_SAFE_FILE_ID < (1 << 24)) };
    }
}
