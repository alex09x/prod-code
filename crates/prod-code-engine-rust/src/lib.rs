//! High-performance, in-memory Rust analysis engine for prod-code directly utilizing `ra_ap_ide::AnalysisHost`.

use anyhow::{Context, Result};
use ra_ap_ide::{
    AnalysisHost, FileId, FilePosition, FileRange, FileStructureConfig, FindAllRefsConfig,
    GotoDefinitionConfig, HoverConfig, HoverDocFormat, RaFixtureConfig, TextRange, TextSize,
};
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{Vfs, VfsPath};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// In-memory Rust analysis engine core backed by a warm Salsa database.
pub struct RustEngine {
    pub workspace_root: PathBuf,
    host: AnalysisHost,
    vfs: Vfs,
}

impl RustEngine {
    /// Detect whether a directory contains a Rust workspace manifest (Cargo.toml).
    pub fn is_rust_workspace(path: &Path) -> bool {
        path.join("Cargo.toml").exists()
    }

    /// Load and index a Cargo workspace directly into in-memory Salsa DB.
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let cargo_config = CargoConfig::default();
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: false,
            num_worker_threads: 0,
            proc_macro_processes: 0,
        };

        tracing::info!(
            ?workspace_root,
            "Loading Cargo workspace into Salsa database"
        );
        let (db, vfs, _proc_macro) =
            load_workspace_at(workspace_root, &cargo_config, &load_config, &|_| {})
                .map_err(|e| anyhow::anyhow!("Failed to load cargo workspace: {e}"))?;

        let host = AnalysisHost::with_database(db);
        tracing::info!(?workspace_root, "Cargo workspace warm and ready in RAM");

        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            host,
            vfs,
        })
    }

    /// Lookup Vfs FileId for a filesystem path.
    pub fn file_id_for_path(&self, path: &Path) -> Option<FileId> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        };
        let vfs_path = VfsPath::new_real_path(abs.to_string_lossy().to_string());
        self.vfs.file_id(&vfs_path).map(|(id, _)| id)
    }

    /// Lookup filesystem path for a Vfs FileId.
    pub fn path_for_file_id(&self, file_id: FileId) -> Option<PathBuf> {
        let vfs_path = self.vfs.file_path(file_id);
        vfs_path.as_path().map(|p| PathBuf::from(p.as_str()))
    }

    /// Retrieve symbol type, docs, and signature at (line, col).
    pub fn hover(&self, path: &Path, line: u32, col: u32) -> Result<Option<String>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;

        let text = self.host.analysis().file_text(file_id)?;
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

        let res = self.host.analysis().hover(&config, file_range)?;
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

        let text = self.host.analysis().file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));

        let file_pos = FilePosition { file_id, offset };
        let config = GotoDefinitionConfig {
            ra_fixture: RaFixtureConfig::default(),
        };

        let targets = match self.host.analysis().goto_definition(file_pos, &config)? {
            Some(range_info) => range_info.info,
            None => return Ok(vec![]),
        };

        let mut results = Vec::new();
        for target in targets {
            if let Some(target_path) = self.path_for_file_id(target.file_id) {
                let target_text = self.host.analysis().file_text(target.file_id)?;
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

        let text = self.host.analysis().file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));

        let file_pos = FilePosition { file_id, offset };
        let config = FindAllRefsConfig {
            search_scope: None,
            ra_fixture: RaFixtureConfig::default(),
            exclude_imports: false,
            exclude_tests: false,
        };

        let search_res = match self.host.analysis().find_all_refs(file_pos, &config)? {
            Some(res) => res,
            None => return Ok(vec![]),
        };

        let mut results = Vec::new();
        for res in search_res {
            for (ref_file_id, refs) in res.references {
                if let (Some(ref_path), Ok(ref_text)) = (
                    self.path_for_file_id(ref_file_id),
                    self.host.analysis().file_text(ref_file_id),
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

        let text = self.host.analysis().file_text(file_id)?;
        let config = FileStructureConfig {
            exclude_locals: false,
        };
        let nodes = self.host.analysis().file_structure(&config, file_id)?;

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

    /// Single-owner fast path: Apply live buffer edits directly into Salsa DB in memory.
    pub fn apply_file_change(&mut self, path: &Path, new_text: String) -> Result<()> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;

        let mut change = ChangeWithProcMacros::default();
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

    #[test]
    fn test_rust_engine_in_memory_queries() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();

        let engine =
            RustEngine::load(workspace_root).expect("Must load workspace directly into Salsa DB");

        // Test document symbols on protocol lib.rs
        let lib_path = workspace_root.join("crates/prod-code-protocol/src/lib.rs");
        let syms = engine
            .document_symbols(&lib_path)
            .expect("Must get document symbols");
        assert!(!syms.is_empty(), "Must find symbols in protocol lib.rs");
        assert!(syms.iter().any(|s| s.name == "DEFAULT_PORT"));

        // Test in-memory hover on DEFAULT_PORT (line 13, col 15)
        let hover = engine
            .hover(&lib_path, 13, 15)
            .expect("Must query in-memory hover");
        assert!(hover.is_some(), "Hover must resolve for DEFAULT_PORT");
        assert!(hover.unwrap().contains("DEFAULT_PORT"));

        // Test in-memory jump to definition for PathTranslator (line 11, col 15)
        let defs = engine
            .goto_definition(&lib_path, 11, 15)
            .expect("Must query definition");
        assert!(
            !defs.is_empty(),
            "Must resolve definition for PathTranslator"
        );
        assert!(defs.iter().any(|d| d.name == "PathTranslator"));
    }

    #[test]
    fn test_rust_engine_direct_mutation() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();

        let mut engine = RustEngine::load(workspace_root).expect("Must load workspace");
        let lib_path = workspace_root.join("crates/prod-code-protocol/src/lib.rs");

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
}
