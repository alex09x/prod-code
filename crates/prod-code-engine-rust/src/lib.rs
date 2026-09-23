//! High-performance, in-memory Rust analysis engine for prod-code directly utilizing `ra_ap_ide::AnalysisHost`.

use anyhow::{Context, Result};
use ra_ap_cfg::{CfgAtom, CfgDiff};
use ra_ap_ide::{
    AnalysisHost, AssistConfig, AssistResolveStrategy, CallHierarchyConfig, DiagnosticsConfig,
    FileId, FilePosition, FileRange, FileStructureConfig, FindAllRefsConfig, GotoDefinitionConfig,
    GotoImplementationConfig, HighlightConfig, HoverConfig, HoverDocFormat, NavigationTarget,
    RaFixtureConfig, RenameConfig, SingleResolve, StructureNodeKind, SymbolKind, TextRange,
    TextSize,
};
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_ide_db::SnippetCap;
use ra_ap_ide_db::source_change::FileSystemEdit;
use ra_ap_ide_db::source_change::SourceChange;
use ra_ap_intern::sym;
use ra_ap_load_cargo::{
    LoadCargoConfig, ProcMacroServerChoice, ProjectFolders, SourceRootConfig, load_workspace_at,
};
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{
    CargoConfig, CargoFeatures, CfgOverrides, ProjectManifest, ProjectWorkspace, RustLibSource,
};
use ra_ap_vfs::AnchoredPathBuf;
use ra_ap_vfs::{Vfs, VfsPath};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Per-repository analysis settings, read from `prod-code.toml` at the workspace root:
///
/// ```toml
/// [rust]
/// features = "all"            # or ["feat-a", "feat-b"]
/// no_default_features = false
/// all_targets = true          # tests, benches and examples are analysed too
/// sysroot = true              # load the standard library sources (rust-src)
/// ```
///
/// Repositories that compile the same source files into several crates behind different
/// feature flags (a `#[path]`-shared module tree) need `features = "all"`: rust-analyzer
/// attaches each file to one crate, and a module behind a disabled feature is dead there.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProdCodeConfig {
    pub rust: RustAnalysisOptions,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RustAnalysisOptions {
    pub features: FeatureSelection,
    pub no_default_features: bool,
    pub all_targets: bool,
    pub sysroot: bool,
    /// Run build scripts (`cargo check` on load, warm on the gateway) so `OUT_DIR` code and
    /// proc-macro crates exist, and expand proc macros through rust-analyzer's out-of-process
    /// proc-macro server. Without it derives and attribute macros resolve to nothing.
    pub build_scripts: bool,
}

impl Default for RustAnalysisOptions {
    fn default() -> Self {
        Self {
            features: FeatureSelection::Selected(Vec::new()),
            no_default_features: false,
            all_targets: true,
            sysroot: true,
            build_scripts: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FeatureSelection {
    /// `features = "all"`.
    Keyword(String),
    /// `features = ["a", "b"]`.
    Selected(Vec<String>),
}

impl ProdCodeConfig {
    /// The configuration file name looked up at the workspace root.
    pub const FILE_NAME: &'static str = "prod-code.toml";

    /// Reads `<root>/prod-code.toml`; a missing file is the default configuration and a
    /// malformed one is reported and ignored.
    pub fn load(root: &Path) -> Self {
        let path = root.join(Self::FILE_NAME);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "prod-code.toml ignored");
                Self::default()
            }
        }
    }

    /// The rust-analyzer cargo configuration these options describe.
    pub fn cargo_config(&self) -> CargoConfig {
        let rust = &self.rust;
        let features = match &rust.features {
            FeatureSelection::Keyword(word) if word.eq_ignore_ascii_case("all") => {
                CargoFeatures::All
            }
            FeatureSelection::Keyword(word) => CargoFeatures::Selected {
                features: vec![word.clone()],
                no_default_features: rust.no_default_features,
            },
            FeatureSelection::Selected(features) => CargoFeatures::Selected {
                features: features.clone(),
                no_default_features: rust.no_default_features,
            },
        };
        CargoConfig {
            all_targets: rust.all_targets,
            features,
            sysroot: rust.sysroot.then_some(RustLibSource::Discover),
            // Like rust-analyzer's `cargo.cfgs` default: `cfg(test)` modules and
            // `debug_assertions` code are analysed, so #[test] functions exist in the
            // call graph and the impact analysis sees them.
            cfg_overrides: CfgOverrides {
                global: CfgDiff::new(
                    vec![
                        CfgAtom::Flag(sym::test),
                        CfgAtom::Flag(sym::debug_assertions),
                    ],
                    Vec::new(),
                ),
                ..CfgOverrides::default()
            },
            ..CargoConfig::default()
        }
    }
}

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

/// A function, method or other item in the call hierarchy: where it is declared (`line`/`col`
/// point at its name, `end_line`/`end_col` close the whole item) and what kind of item it is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HierarchyItem {
    pub name: String,
    pub kind: String,
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// One edge of the call graph: the caller or callee item and the 1-based positions of the
/// call sites (in the caller's file for incoming calls, in this item's file for outgoing).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallEdge {
    pub item: HierarchyItem,
    pub call_sites: Vec<(u32, u32)>,
    /// The caller is a test (`#[test]` or inside a `cfg(test)` module), as rust-analyzer
    /// classifies it.
    #[serde(default)]
    pub is_test: bool,
}

/// One rust-analyzer diagnostic for a file, computed in memory (no cargo check).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDiagnostic {
    pub code: String,
    pub message: String,
    /// `error`, `warning`, `weak` (hint) or `allow`.
    pub severity: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub unused: bool,
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
    /// 1-based line of the item's name.
    pub line: u32,
    /// 1-based column of the item's name.
    #[serde(default)]
    pub col: u32,
    /// 1-based last line of the whole item (its body included).
    #[serde(default)]
    pub end_line: u32,
    pub detail: Option<String>,
    /// Labels of the enclosing items, outermost first (`mod tests`, `impl Shape for Circle`).
    #[serde(default)]
    pub containers: Vec<String>,
}

/// A file rewritten by a refactoring: its full new content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewrittenFile {
    pub path: PathBuf,
    pub new_text: String,
    /// Number of individual text edits folded into `new_text`.
    pub edits: usize,
    /// Line count of the previous content (for whole-file replacement ranges).
    pub old_line_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Everything a refactoring changes: rewritten files, new files, and moves/renames of files
/// or directories (a module rename renames its file).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefactorOutcome {
    pub files: Vec<RewrittenFile>,
    pub created: Vec<RewrittenFile>,
    pub moves: Vec<FileMove>,
}

impl RefactorOutcome {
    pub fn total_edits(&self) -> usize {
        self.files.iter().map(|f| f.edits).sum()
    }
}

/// A code action rust-analyzer offers at a position or selection (inline, extract, generate,
/// rewrite, quick fix). `id` plus `subtype` identify it for [`RustEngineSnapshot::apply_assist`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistInfo {
    pub id: String,
    pub kind: String,
    pub subtype: Option<usize>,
    pub label: String,
    pub group: Option<String>,
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

    /// Renames the symbol at 1-based (line, col) to `new_name` across the workspace.
    /// `Ok(Err(reason))` means rust-analyzer refused (no symbol there, invalid name, conflict).
    pub fn rename(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;
        let text = self.analysis.file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));
        let position = FilePosition { file_id, offset };
        let config = RenameConfig {
            show_conflicts: true,
            prefer_no_std: false,
            prefer_prelude: true,
            prefer_absolute: false,
        };
        let change = match self.analysis.rename(position, new_name, &config)? {
            Ok(change) => change,
            Err(refused) => return Ok(Err(refused.to_string())),
        };
        Ok(Ok(self.outcome_from_change(&change)?))
    }

    /// Structural search and replace across the workspace (roadmap 8.7).
    ///
    /// `rule` is rust-analyzer's own SSR syntax, `pattern ==>> replacement`, where `$name`
    /// is a placeholder bound by the match: `$a.unwrap() ==>> $a.expect("...")`. Matching is
    /// on the syntax tree with name resolution, not on text, so a call written across three
    /// lines matches and a comment that looks like the pattern does not.
    ///
    /// A position is needed to resolve the paths the pattern mentions; any file of the
    /// workspace will do, and the caller passes the one the agent was looking at.
    pub fn structural_replace(
        &self,
        rule: &str,
        context: &Path,
        line: u32,
        col: u32,
        scope: Option<&Path>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let file_id = self
            .file_id_for_path(context)
            .with_context(|| format!("File not found in VFS: {:?}", context))?;
        let text = self.analysis.file_text(file_id)?;
        // The pattern's paths are resolved as if they appeared at this position, so it has to
        // be inside an item: at offset 0 a file has no scope and nothing resolves, which looks
        // exactly like "no matches". When the caller has no position of its own, use the body
        // of the file's first function.
        let offset = match line_col_to_offset(&text, line, col) {
            Some(offset) if line > 1 || col > 1 => offset,
            _ => self.first_body_offset(file_id).unwrap_or(TextSize::from(0)),
        };
        let position = FilePosition { file_id, offset };
        // Without a scope the search covers the workspace and every crate it depends on,
        // which means a usage search with type inference per crate: minutes on a large
        // workspace. A scope restricts it to one file and makes it interactive.
        let selections = match scope {
            None => Vec::new(),
            Some(path) => {
                let scope_id = self
                    .file_id_for_path(path)
                    .with_context(|| format!("File not found in VFS: {:?}", path))?;
                let scope_text = self.analysis.file_text(scope_id)?;
                vec![FileRange {
                    file_id: scope_id,
                    range: TextRange::new(
                        TextSize::from(0),
                        TextSize::from(scope_text.len() as u32),
                    ),
                }]
            }
        };
        tracing::info!(
            rule,
            context = %context.display(),
            offset = u32::from(offset),
            scoped = selections.len(),
            "🔧 [SSR] resolving"
        );
        let change = match self
            .analysis
            .structural_search_replace(rule, false, position, selections)?
        {
            Ok(change) => change,
            Err(e) => {
                tracing::info!(rule, error = %e, "🔧 [SSR] refused");
                return Ok(Err(e.to_string()));
            }
        };
        let outcome = self.outcome_from_change(&change)?;
        tracing::info!(
            rule,
            files = outcome.files.len(),
            edits = outcome.total_edits(),
            "🔧 [SSR] done"
        );
        Ok(Ok(outcome))
    }

    /// An offset inside the first function of `file_id`, for resolving a pattern's paths.
    fn first_body_offset(&self, file_id: FileId) -> Option<TextSize> {
        let structure = self
            .analysis
            .file_structure(
                &FileStructureConfig {
                    exclude_locals: true,
                },
                file_id,
            )
            .ok()?;
        let node = structure.iter().find(|n| {
            matches!(
                n.kind,
                StructureNodeKind::SymbolKind(ra_ap_ide::SymbolKind::Function)
            )
        })?;
        // Just inside the item: past its name, where its body scope is in effect.
        Some(node.node_range.start() + TextSize::from(1))
    }

    /// Turns a rust-analyzer `SourceChange` into rewritten files, created files and moves.
    fn outcome_from_change(&self, change: &SourceChange) -> Result<RefactorOutcome> {
        let mut outcome = RefactorOutcome::default();
        for (edited_id, (edit, _snippet)) in change.source_file_edits.iter() {
            let Some(edited_path) = self.path_for_file_id(*edited_id) else {
                continue;
            };
            let old = self.analysis.file_text(*edited_id)?;
            let mut new_text = old.to_string();
            let edits = edit.iter().count();
            edit.apply(&mut new_text);
            outcome.files.push(RewrittenFile {
                path: edited_path,
                new_text,
                edits,
                old_line_count: old.lines().count() as u32,
            });
        }
        for fs_edit in &change.file_system_edits {
            match fs_edit {
                FileSystemEdit::MoveFile { src, dst } => {
                    if let (Some(from), Some(to)) =
                        (self.path_for_file_id(*src), self.anchored_path(dst))
                    {
                        outcome.moves.push(FileMove { from, to });
                    }
                }
                FileSystemEdit::MoveDir { src, dst, .. } => {
                    if let (Some(from), Some(to)) =
                        (self.anchored_path(src), self.anchored_path(dst))
                    {
                        outcome.moves.push(FileMove { from, to });
                    }
                }
                FileSystemEdit::CreateFile {
                    dst,
                    initial_contents,
                } => {
                    if let Some(path) = self.anchored_path(dst) {
                        outcome.created.push(RewrittenFile {
                            path,
                            new_text: initial_contents.clone(),
                            edits: 1,
                            old_line_count: 0,
                        });
                    }
                }
            }
        }
        outcome.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(outcome)
    }

    fn assist_configs() -> (AssistConfig, DiagnosticsConfig) {
        let diagnostics = DiagnosticsConfig::test_sample();
        let assists = AssistConfig {
            snippet_cap: SnippetCap::new(false),
            allowed: None,
            insert_use: diagnostics.insert_use,
            prefer_no_std: false,
            prefer_prelude: true,
            prefer_absolute: false,
            assist_emit_must_use: false,
            term_search_fuel: 1800,
            term_search_borrowck: true,
            code_action_grouping: true,
            expr_fill_default: Default::default(),
            prefer_self_ty: false,
            show_rename_conflicts: true,
        };
        (assists, diagnostics)
    }

    fn file_range(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
    ) -> Result<FileRange> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;
        let text = self.analysis.file_text(file_id)?;
        let start = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));
        let end = end
            .and_then(|(l, c)| line_col_to_offset(&text, l, c))
            .unwrap_or(start);
        let (start, end) = if end < start {
            (end, start)
        } else {
            (start, end)
        };
        Ok(FileRange {
            file_id,
            range: TextRange::new(start, end),
        })
    }

    /// Code actions available at a 1-based position or selection (`end` inclusive-exclusive).
    pub fn list_assists(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
    ) -> Result<Vec<AssistInfo>> {
        let frange = self.file_range(path, line, col, end)?;
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let assists = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            AssistResolveStrategy::None,
            frange,
        )?;
        Ok(assists
            .into_iter()
            .map(|a| AssistInfo {
                id: a.id.0.to_string(),
                kind: format!("{:?}", a.id.1),
                subtype: a.id.2,
                label: a.label.to_string(),
                group: a.group.map(|g| g.0),
            })
            .collect())
    }

    /// Computes the edit of one assist (by `id` and optional `subtype`) at the position.
    /// `Ok(Err(reason))` when no such assist is offered there.
    pub fn apply_assist(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
        id: &str,
        subtype: Option<usize>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let frange = self.file_range(path, line, col, end)?;
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let offered = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            AssistResolveStrategy::None,
            frange,
        )?;
        let Some(target) = offered
            .iter()
            .find(|a| a.id.0 == id && (subtype.is_none() || a.id.2 == subtype))
        else {
            let available: Vec<String> = offered.iter().map(|a| a.id.0.to_string()).collect();
            let available = if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            };
            // Inside a macro call's input rust-analyzer sees tokens more than code, and most
            // refactorings are not offered there; "not offered here" alone leaves the reader to
            // guess why (#99).
            if let Some(mac) = self.macro_call_around(frange) {
                return Ok(Err(format!(
                    "assist `{id}` is not offered here: the selection is inside `{mac}!`, and \
                     rust-analyzer does not offer `{id}` inside a macro call's input. Move the \
                     code out of the macro (into a function the macro calls) first. Available \
                     here: {available}"
                )));
            }
            return Ok(Err(format!(
                "assist `{id}` is not offered here; available: {available}"
            )));
        };
        let resolve = AssistResolveStrategy::Single(SingleResolve {
            assist_id: target.id.0.to_string(),
            assist_kind: target.id.1,
            assist_subtype: target.id.2,
        });
        let resolved = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            resolve,
            frange,
        )?;
        let Some(change) = resolved
            .into_iter()
            .find(|a| a.id.0 == id && (subtype.is_none() || a.id.2 == subtype))
            .and_then(|a| a.source_change)
        else {
            return Ok(Err(format!("assist `{id}` produced no edit")));
        };
        Ok(Ok(self.outcome_from_change(&change)?))
    }

    /// The path of the innermost macro call whose input contains `frange`, if any.
    fn macro_call_around(&self, frange: FileRange) -> Option<String> {
        use ra_ap_syntax::AstNode;
        let file = self.analysis.parse(frange.file_id).ok()?;
        let token = file
            .syntax()
            .token_at_offset(frange.range.start())
            .right_biased()?;
        token
            .parent_ancestors()
            .filter_map(ra_ap_syntax::ast::MacroCall::cast)
            .find(|call| {
                call.token_tree()
                    .is_some_and(|tree| tree.syntax().text_range().contains_range(frange.range))
            })
            .and_then(|call| call.path())
            .map(|path| path.syntax().text().to_string())
    }

    /// Deletes the item (function, type, const, field, module …) whose name is at the 1-based
    /// position, but only when nothing else in the workspace references it.
    /// `Ok(Err(dossier))` lists the usages that block the deletion.
    pub fn safe_delete(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let usages = self.find_all_refs(path, line, col)?;
        if !usages.is_empty() {
            let mut listed: Vec<String> = usages
                .iter()
                .take(20)
                .map(|u| format!("{}:{}:{}", u.path.display(), u.line, u.col))
                .collect();
            if usages.len() > listed.len() {
                listed.push(format!("… {} more", usages.len() - listed.len()));
            }
            return Ok(Err(format!(
                "{} usage(s) reference this item; delete refused:\n{}",
                usages.len(),
                listed.join("\n")
            )));
        }
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;
        let text = self.analysis.file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));
        let config = FileStructureConfig {
            exclude_locals: false,
        };
        let nodes = self.analysis.file_structure(&config, file_id)?;
        // Only the item whose name is at the position. Falling back to the smallest item that
        // merely contains it deleted a line of a function's body for a position on a parameter
        // (#138): a position that names no item is refused instead.
        let node = nodes
            .iter()
            .filter(|n| n.navigation_range.contains_inclusive(offset))
            .min_by_key(|n| n.node_range.len());
        let Some(node) = node else {
            return Ok(Err(
                "no deletable item is named at this position; give the position of an item's \
                 name (a parameter is removed with `code_change_signature`)"
                    .to_string(),
            ));
        };
        let start = usize::from(node.node_range.start());
        let mut end = usize::from(node.node_range.end());
        // Take the line terminator with the item, and one blank line if that leaves two.
        if text[end..].starts_with('\n') {
            end += 1;
        }
        let mut new_text = String::with_capacity(text.len());
        new_text.push_str(&text[..start]);
        new_text.push_str(&text[end..]);
        let new_text = new_text.replace("\n\n\n", "\n\n");
        Ok(Ok(RefactorOutcome {
            files: vec![RewrittenFile {
                path: normalize_vfs_path(path, &self.workspace_root),
                new_text,
                edits: 1,
                old_line_count: text.lines().count() as u32,
            }],
            created: Vec::new(),
            moves: Vec::new(),
        }))
    }

    fn anchored_path(&self, anchored: &AnchoredPathBuf) -> Option<PathBuf> {
        let anchor = self.path_for_file_id(anchored.anchor)?;
        Some(anchor.parent()?.join(&anchored.path))
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
    fn file_position(&self, path: &Path, line: u32, col: u32) -> Result<FilePosition> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;
        let text = self.analysis.file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col).unwrap_or(TextSize::from(0));
        Ok(FilePosition { file_id, offset })
    }

    fn hierarchy_item(&self, target: &NavigationTarget) -> Option<HierarchyItem> {
        let path = self.path_for_file_id(target.file_id)?;
        let text = self.analysis.file_text(target.file_id).ok()?;
        let focus = target.focus_range.unwrap_or(target.full_range);
        let (line, col) = offset_to_line_col(&text, focus.start());
        let (end_line, end_col) = offset_to_line_col(&text, target.full_range.end());
        Some(HierarchyItem {
            name: target.name.to_string(),
            kind: target
                .kind
                .map(|k| format!("{k:?}"))
                .unwrap_or_else(|| "Function".to_string()),
            path,
            line,
            col,
            end_line,
            end_col,
        })
    }

    /// All diagnostics rust-analyzer computes for `path` from the in-memory database: syntax
    /// errors, unresolved names, type mismatches, unused items and the like. This is what a
    /// proposed edit can be validated against before it touches disk.
    pub fn diagnostics(&self, path: &Path) -> Result<Vec<FileDiagnostic>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;
        let text = self.analysis.file_text(file_id)?;
        let config = DiagnosticsConfig::test_sample();
        let diagnostics =
            self.analysis
                .full_diagnostics(&config, AssistResolveStrategy::None, file_id)?;
        let unused_imports = self.unused_imports(file_id, &text);
        Ok(diagnostics
            .into_iter()
            .filter(|d| d.range.file_id == file_id)
            .map(|d| {
                let (line, col) = offset_to_line_col(&text, d.range.range.start());
                let (end_line, end_col) = offset_to_line_col(&text, d.range.range.end());
                let severity = format!("{:?}", d.severity).to_ascii_lowercase();
                FileDiagnostic {
                    code: d.code.as_str().to_string(),
                    message: d.message,
                    severity: match severity.as_str() {
                        "weakwarning" => "weak".to_string(),
                        other => other.to_string(),
                    },
                    line,
                    col,
                    end_line,
                    end_col,
                    unused: d.unused,
                }
            })
            .chain(unused_imports)
            .collect())
    }

    /// The `use` items of `file_id` that import something unused (#134). rust-analyzer computes
    /// no diagnostic for an unused import — rustc does, and `-D warnings` rejects it — but it
    /// offers `remove_unused_imports` exactly on a `use` item that has one. So each `use` item is
    /// asked for its assists, and the ones that offer it are reported as rustc would.
    fn unused_imports(&self, file_id: FileId, text: &str) -> Vec<FileDiagnostic> {
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let mut out = Vec::new();
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            let item_start = offset + (line.len() - line.trim_start().len());
            offset += line.len();
            let Some(path_start) = use_path_start(&text[item_start..]) else {
                continue;
            };
            let at = TextSize::from((item_start + path_start) as u32);
            let range = FileRange {
                file_id,
                range: TextRange::empty(at),
            };
            let Ok(assists) = self.analysis.assists_with_fixes(
                &assist_config,
                &diagnostics_config,
                AssistResolveStrategy::None,
                range,
            ) else {
                continue;
            };
            if !assists.iter().any(|a| a.id.0 == "remove_unused_imports") {
                continue;
            }
            let end = text[item_start..]
                .find(';')
                .map_or(text.len(), |i| item_start + i + 1);
            let (line_no, col) = offset_to_line_col(text, TextSize::from(item_start as u32));
            let (end_line, end_col) = offset_to_line_col(text, TextSize::from(end as u32));
            out.push(FileDiagnostic {
                code: "unused_imports".to_string(),
                message: "unused import".to_string(),
                severity: "warning".to_string(),
                line: line_no,
                col,
                end_line,
                end_col,
                unused: true,
            });
        }
        out
    }

    /// The call-hierarchy item(s) at (line, col): the enclosing or referenced function.
    pub fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<HierarchyItem>> {
        let pos = self.file_position(path, line, col)?;
        let config = CallHierarchyConfig {
            exclude_tests: false,
            ra_fixture: RaFixtureConfig::default(),
        };
        let targets = match self.analysis.call_hierarchy(pos, &config)? {
            Some(info) => info.info,
            None => return Ok(vec![]),
        };
        Ok(targets
            .iter()
            .filter_map(|t| self.hierarchy_item(t))
            .collect())
    }

    /// Everything that calls the function whose name is at (line, col).
    pub fn incoming_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        let pos = self.file_position(path, line, col)?;
        let config = CallHierarchyConfig {
            exclude_tests: false,
            ra_fixture: RaFixtureConfig::default(),
        };
        let calls = self
            .analysis
            .incoming_calls(&config, pos)?
            .unwrap_or_default();
        let mut edges = self.call_edges(calls);
        // Callers that disappear when tests are excluded are the tests.
        let without_tests = CallHierarchyConfig {
            exclude_tests: true,
            ra_fixture: RaFixtureConfig::default(),
        };
        let non_test: std::collections::HashSet<(PathBuf, u32, u32)> = self
            .call_edges(
                self.analysis
                    .incoming_calls(&without_tests, pos)?
                    .unwrap_or_default(),
            )
            .into_iter()
            .map(|e| (e.item.path, e.item.line, e.item.col))
            .collect();
        for edge in &mut edges {
            edge.is_test =
                !non_test.contains(&(edge.item.path.clone(), edge.item.line, edge.item.col));
        }
        Ok(edges)
    }

    /// Everything the function whose name is at (line, col) calls.
    pub fn outgoing_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        let pos = self.file_position(path, line, col)?;
        let config = CallHierarchyConfig {
            exclude_tests: false,
            ra_fixture: RaFixtureConfig::default(),
        };
        let calls = self
            .analysis
            .outgoing_calls(&config, pos)?
            .unwrap_or_default();
        Ok(self.call_edges(calls))
    }

    fn call_edges(&self, calls: Vec<ra_ap_ide::CallItem>) -> Vec<CallEdge> {
        calls
            .iter()
            .filter_map(|call| {
                let item = self.hierarchy_item(&call.target)?;
                let call_sites = call
                    .ranges
                    .iter()
                    .filter_map(|range| {
                        let text = self.analysis.file_text(range.file_id).ok()?;
                        Some(offset_to_line_col(&text, range.range.start()))
                    })
                    .collect();
                Some(CallEdge {
                    item,
                    call_sites,
                    is_test: false,
                })
            })
            .collect()
    }

    /// All implementations of the trait, or the impl blocks of the type, at (line, col).
    pub fn goto_implementation(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        let pos = self.file_position(path, line, col)?;
        let config = GotoImplementationConfig {
            filter_adjacent_derive_implementations: false,
        };
        let targets = match self.analysis.goto_implementation(&config, pos)? {
            Some(info) => info.info,
            None => return Ok(vec![]),
        };
        Ok(targets
            .iter()
            .filter_map(|t| {
                let item = self.hierarchy_item(t)?;
                Some(DefinitionTarget {
                    path: item.path,
                    line: item.line,
                    col: item.col,
                    name: item.name,
                })
            })
            .collect())
    }

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

        let labels: Vec<String> = nodes.iter().map(|n| n.label.clone()).collect();
        let parents: Vec<Option<usize>> = nodes.iter().map(|n| n.parent).collect();
        let mut results = Vec::new();
        for node in nodes {
            let mut containers = Vec::new();
            let mut parent = node.parent;
            while let Some(index) = parent {
                if let Some(label) = labels.get(index) {
                    containers.push(label.clone());
                }
                parent = parents.get(index).copied().flatten();
                if containers.len() > 16 {
                    break;
                }
            }
            containers.reverse();
            // The navigation range is the item's name; the node range would start at its
            // doc comments and attributes.
            let (sym_line, sym_col) = offset_to_line_col(&text, node.navigation_range.start());
            let (end_line, _) = offset_to_line_col(&text, node.node_range.end());
            results.push(SymbolTarget {
                name: node.label,
                kind: match node.kind {
                    StructureNodeKind::SymbolKind(kind) => format!("{kind:?}"),
                    other => format!("{other:?}"),
                },
                line: sym_line,
                col: sym_col,
                end_line: end_line.max(sym_line),
                detail: node.detail,
                containers,
            });
        }

        Ok(results)
    }

    /// Workspace-wide symbol search by name (what `workspace/symbol` answers): fuzzy on the
    /// name, workspace crates only, associated items included. Positions point at the name.
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Result<Vec<WorkspaceSymbol>> {
        let query = ra_ap_ide::Query::new(query.to_string());
        let targets = self.analysis.symbol_search(query, limit.max(1))?;
        let mut out = Vec::with_capacity(targets.len());
        for target in targets {
            let Some(path) = self.path_for_file_id(target.file_id) else {
                continue;
            };
            let Ok(text) = self.analysis.file_text(target.file_id) else {
                continue;
            };
            let focus = target.focus_range.unwrap_or(target.full_range);
            let (line, col) = offset_to_line_col(&text, focus.start());
            let (end_line, _) = offset_to_line_col(&text, target.full_range.end());
            out.push(WorkspaceSymbol {
                path,
                name: target.name.to_string(),
                kind: target
                    .kind
                    .map(|k| format!("{k:?}"))
                    .unwrap_or_else(|| "Symbol".to_string()),
                line,
                col,
                end_line: end_line.max(line),
                container: target.container_name.map(|c| c.to_string()),
            });
        }
        Ok(out)
    }
}

/// One hit of a workspace-wide symbol search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSymbol {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    /// 1-based line and column of the name.
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    /// Enclosing item (`impl` self type, module, trait), when the index knows it.
    pub container: Option<String>,
}

/// In-memory Rust analysis engine core backed by a warm Salsa database.
pub struct RustEngine {
    pub workspace_root: PathBuf,
    host: AnalysisHost,
    vfs: Arc<std::sync::RwLock<Vfs>>,
    source_root_config: Arc<SourceRootConfig>,
    overlays: SessionOverlays,
}

/// Per-session live buffers layered over the shared base workspace.
///
/// Several sessions (agent worktrees) share one Salsa database. Each session's `didOpen` /
/// `didChange` / dirty-file sync goes into its own overlay instead of the shared base, and a
/// query first activates its session so the database reflects exactly that session's buffers.
/// `owner` records whose text currently sits in the database for a path; `base` keeps the text
/// the database held before the first overlay touched that path (`None` = file absent).
#[derive(Default)]
struct SessionOverlays {
    sessions: HashMap<u64, HashMap<PathBuf, Option<String>>>,
    owner: HashMap<PathBuf, u64>,
    base: HashMap<PathBuf, Option<String>>,
}

impl RustEngine {
    /// Text the database currently holds for `norm`, or `None` when the file is unknown.
    fn current_db_text(&self, norm: &Path) -> Option<String> {
        let file_id = self.file_id_for_path(norm)?;
        self.host
            .analysis()
            .file_text(file_id)
            .ok()
            .map(|text| text.to_string())
    }

    /// Writes `text` for `norm` into the database; `None` empties the file (rust-analyzer's
    /// representation of a deleted file) without touching the VFS registration.
    fn apply_text(&mut self, norm: &Path, text: Option<String>) -> Result<()> {
        match text {
            Some(text) => self.apply_file_change(norm, text),
            None => {
                if let Some(file_id) = self.file_id_for_path(norm) {
                    let mut change = ChangeWithProcMacros::default();
                    change.change_file(file_id, None);
                    self.host.apply_change(change);
                }
                Ok(())
            }
        }
    }

    fn restore_base(&mut self, norm: &Path) -> Result<()> {
        let base = self.overlays.base.get(norm).cloned().flatten();
        self.apply_text(norm, base)?;
        self.overlays.owner.remove(norm);
        let still_referenced = self
            .overlays
            .sessions
            .values()
            .any(|files| files.contains_key(norm));
        if !still_referenced {
            self.overlays.base.remove(norm);
        }
        Ok(())
    }

    /// Records `text` (or a deletion) as `session`'s private view of `path`. The database is
    /// updated immediately when nobody else's buffer currently occupies that path.
    pub fn set_session_overlay(
        &mut self,
        session: u64,
        path: &Path,
        text: Option<String>,
    ) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        if !self.overlays.base.contains_key(&norm) {
            let base = self.current_db_text(&norm);
            self.overlays.base.insert(norm.clone(), base);
        }
        self.overlays
            .sessions
            .entry(session)
            .or_default()
            .insert(norm.clone(), text.clone());
        match self.overlays.owner.get(&norm) {
            Some(owner) if *owner != session => Ok(()),
            _ => {
                self.apply_text(&norm, text)?;
                self.overlays.owner.insert(norm, session);
                Ok(())
            }
        }
    }

    /// Drops `session`'s buffer for `path`, restoring the shared base text if that buffer was
    /// the one in the database.
    pub fn clear_session_overlay(&mut self, session: u64, path: &Path) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let removed = self
            .overlays
            .sessions
            .get_mut(&session)
            .map(|files| files.remove(&norm).is_some())
            .unwrap_or(false);
        if !removed {
            return Ok(());
        }
        if self.overlays.owner.get(&norm) == Some(&session) {
            self.restore_base(&norm)?;
        } else {
            let still_referenced = self
                .overlays
                .sessions
                .values()
                .any(|files| files.contains_key(&norm));
            if !still_referenced && !self.overlays.owner.contains_key(&norm) {
                self.overlays.base.remove(&norm);
            }
        }
        if self
            .overlays
            .sessions
            .get(&session)
            .is_some_and(|files| files.is_empty())
        {
            self.overlays.sessions.remove(&session);
        }
        Ok(())
    }

    /// Drops every buffer of `session` (session teardown).
    pub fn clear_session(&mut self, session: u64) -> Result<()> {
        let paths: Vec<PathBuf> = self
            .overlays
            .sessions
            .get(&session)
            .map(|files| files.keys().cloned().collect())
            .unwrap_or_default();
        for norm in paths {
            self.clear_session_overlay(session, &norm)?;
        }
        Ok(())
    }

    /// Makes the database reflect `session`'s view: its own buffers are applied and every other
    /// session's buffer on a path this session has not opened is replaced by the base text.
    /// Returns the number of files rewritten. Must run before every query of that session, under
    /// the same lock as the query, so no other session can switch the view in between.
    pub fn activate_session(&mut self, session: u64) -> Result<usize> {
        let mut switched = 0;
        let mine: HashMap<PathBuf, Option<String>> = self
            .overlays
            .sessions
            .get(&session)
            .cloned()
            .unwrap_or_default();

        let foreign: Vec<PathBuf> = self
            .overlays
            .owner
            .iter()
            .filter(|(norm, owner)| **owner != session && !mine.contains_key(*norm))
            .map(|(norm, _)| norm.clone())
            .collect();
        for norm in foreign {
            let base = self.overlays.base.get(&norm).cloned().flatten();
            self.apply_text(&norm, base)?;
            self.overlays.owner.remove(&norm);
            switched += 1;
        }

        for (norm, text) in mine {
            if self.overlays.owner.get(&norm) == Some(&session) {
                continue;
            }
            self.apply_text(&norm, text)?;
            self.overlays.owner.insert(norm, session);
            switched += 1;
        }
        Ok(switched)
    }

    /// Records new shared base text for `path` (a workspace sync landed on disk). It is applied
    /// immediately unless some session's buffer currently occupies the path; that session keeps
    /// its view and the new base becomes visible once its buffer is cleared.
    pub fn update_base(&mut self, path: &Path, text: Option<String>) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        if self.overlays.base.contains_key(&norm) {
            self.overlays.base.insert(norm.clone(), text.clone());
        }
        if self.overlays.owner.contains_key(&norm) {
            return Ok(());
        }
        self.apply_text(&norm, text)
    }

    /// Drops every buffer of `session` whose path is not in `keep`: a sync that announces the
    /// session's complete dirty set makes any other overlay of that session stale. Returns the
    /// number of buffers dropped.
    pub fn retain_session_overlays(&mut self, session: u64, keep: &[PathBuf]) -> Result<usize> {
        let keep: HashSet<PathBuf> = keep
            .iter()
            .map(|path| normalize_vfs_path(path, &self.workspace_root))
            .collect();
        let stale: Vec<PathBuf> = self
            .overlays
            .sessions
            .get(&session)
            .map(|files| {
                files
                    .keys()
                    .filter(|norm| !keep.contains(*norm))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for norm in &stale {
            self.clear_session_overlay(session, norm)?;
        }
        Ok(stale.len())
    }

    /// Number of buffers `session` currently overlays.
    pub fn session_overlay_count(&self, session: u64) -> usize {
        self.overlays
            .sessions
            .get(&session)
            .map(|files| files.len())
            .unwrap_or(0)
    }

    /// Detect whether a directory contains a Rust workspace manifest (Cargo.toml).
    pub fn is_rust_workspace(path: &Path) -> bool {
        path.join("Cargo.toml").exists()
    }

    /// Load and index a Cargo workspace directly into in-memory Salsa DB using multi-core worker threads.
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let config = ProdCodeConfig::load(workspace_root);
        tracing::info!(?workspace_root, rust = ?config.rust, "analysis options");
        let cargo_config = config.cargo_config();
        let build_scripts = config.rust.build_scripts;
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: build_scripts,
            with_proc_macro_server: if build_scripts {
                ProcMacroServerChoice::Sysroot
            } else {
                ProcMacroServerChoice::None
            },
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
            overlays: SessionOverlays::default(),
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

    pub fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<HierarchyItem>> {
        self.snapshot().prepare_call_hierarchy(path, line, col)
    }

    pub fn diagnostics(&self, path: &Path) -> Result<Vec<FileDiagnostic>> {
        let started = std::time::Instant::now();
        self.infer_functions_in_parallel(path);
        let primed = started.elapsed();
        let result = self.snapshot().diagnostics(path);
        tracing::debug!(
            file = %path.display(),
            primed_ms = primed.as_millis() as u64,
            diagnostics_ms = (started.elapsed() - primed).as_millis() as u64,
            "diagnostics: functions inferred in parallel, then the diagnostics pass"
        );
        result
    }

    /// Type-checks the functions of `path` on several threads before its diagnostics are asked
    /// for (#86).
    ///
    /// Diagnostics need every body in the file inferred, and rust-analyzer infers them one after
    /// another. After a change to what the crate declares — the usual case for a proposal a
    /// refactoring validates — every body must be inferred again, which for a large file is tens
    /// of seconds on one core of a machine that has dozens. Highlighting a range resolves every
    /// name in it, and so infers the bodies there; doing that for each function on its own
    /// snapshot lets Salsa compute those inferences side by side, and the diagnostics pass that
    /// follows finds them done. A function is inferred by one thread at a time, so a file that is
    /// one huge function gains nothing. The snapshots are used and dropped before this returns:
    /// a snapshot kept alive would block the next write to the database.
    fn infer_functions_in_parallel(&self, path: &Path) {
        let Some(file_id) = self.file_id_for_path(path) else {
            return;
        };
        let analysis = self.host.analysis();
        let Ok(nodes) = analysis.file_structure(
            &FileStructureConfig {
                exclude_locals: true,
            },
            file_id,
        ) else {
            return;
        };
        let mut ranges: Vec<TextRange> = nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    StructureNodeKind::SymbolKind(SymbolKind::Function | SymbolKind::Method)
                )
            })
            .map(|n| n.node_range)
            .collect();
        drop(analysis);
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(16)
            .min(ranges.len());
        if threads < 2 {
            return;
        }
        // Largest first, dealt out in turn, so no thread is left with all the big ones.
        ranges.sort_by_key(|r| std::cmp::Reverse(r.len()));
        let mut shares: Vec<Vec<TextRange>> = vec![Vec::new(); threads];
        for (i, range) in ranges.into_iter().enumerate() {
            shares[i % threads].push(range);
        }
        // Each thread owns its snapshot; all of them are joined, and so dropped, before this
        // returns.
        let workers: Vec<_> = shares
            .into_iter()
            .map(|share| {
                let analysis = self.host.analysis();
                std::thread::spawn(move || {
                    for range in share {
                        let config = HighlightConfig {
                            strings: false,
                            comments: false,
                            punctuation: false,
                            specialize_punctuation: false,
                            operator: false,
                            specialize_operator: false,
                            inject_doc_comment: false,
                            macro_bang: false,
                            syntactic_name_ref_highlighting: false,
                            ra_fixture: RaFixtureConfig::default(),
                        };
                        if analysis
                            .highlight_range(config, FileRange { file_id, range })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            let _ = worker.join();
        }
    }

    pub fn incoming_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        self.snapshot().incoming_calls(path, line, col)
    }

    pub fn outgoing_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        self.snapshot().outgoing_calls(path, line, col)
    }

    pub fn goto_implementation(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        self.snapshot().goto_implementation(path, line, col)
    }

    /// Code actions at a position; see [`RustEngineSnapshot::list_assists`].
    pub fn list_assists(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
    ) -> Result<Vec<AssistInfo>> {
        self.snapshot().list_assists(path, line, col, end)
    }

    /// Apply one code action; see [`RustEngineSnapshot::apply_assist`].
    pub fn apply_assist(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
        id: &str,
        subtype: Option<usize>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot()
            .apply_assist(path, line, col, end, id, subtype)
    }

    /// Delete an unreferenced item; see [`RustEngineSnapshot::safe_delete`].
    pub fn safe_delete(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot().safe_delete(path, line, col)
    }

    /// Rename the symbol at (line, col); see [`RustEngineSnapshot::rename`].
    pub fn rename(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot().rename(path, line, col, new_name)
    }

    pub fn structural_replace(
        &self,
        rule: &str,
        context: &Path,
        line: u32,
        col: u32,
        scope: Option<&Path>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot()
            .structural_replace(rule, context, line, col, scope)
    }

    /// Generate outline / document symbols for a file.
    pub fn document_symbols(&self, path: &Path) -> Result<Vec<SymbolTarget>> {
        self.snapshot().document_symbols(path)
    }

    /// Workspace-wide symbol search by name; see the snapshot method.
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Result<Vec<WorkspaceSymbol>> {
        self.snapshot().workspace_symbols(query, limit)
    }

    /// Single-owner fast path: Apply live buffer edits directly into Salsa DB in memory.
    pub fn apply_file_change(&mut self, path: &Path, new_text: String) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let vfs_path = VfsPath::new_real_path(norm.to_string_lossy().to_string());
        let (file_id, is_new) = if let Some(fid) = self.file_id_for_path(path) {
            // Identical text (the common didOpen of an unmodified file) must not bump the
            // Salsa revision: that would invalidate every derived query for nothing and turn a
            // cached 1 ms hover into a 10-40 ms recomputation.
            if self
                .host
                .analysis()
                .file_text(fid)
                .is_ok_and(|current| *current == *new_text)
            {
                return Ok(());
            }
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

/// Where the path of a `use` item starts, when `item` (the text from the item's first
/// non-blank character on) is one: `use a::b;`, `pub use`, `pub(crate) use`.
fn use_path_start(item: &str) -> Option<usize> {
    let mut rest = item;
    if let Some(after) = rest.strip_prefix("pub") {
        rest = match after.strip_prefix('(') {
            Some(scoped) => &scoped[scoped.find(')')? + 1..],
            None if after.starts_with(char::is_whitespace) => after,
            None => return None,
        };
        rest = rest.trim_start();
    }
    let after_use = rest.strip_prefix("use")?;
    let path = after_use.trim_start();
    (after_use.starts_with(char::is_whitespace) && !path.is_empty())
        .then(|| item.len() - path.len())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_use_item_is_found_behind_its_visibility() {
        use super::use_path_start;
        assert_eq!(use_path_start("use std::fmt;"), Some(4));
        assert_eq!(use_path_start("pub use sync::scan;"), Some(8));
        assert_eq!(use_path_start("pub(crate) use a::b;"), Some(15));
        assert_eq!(use_path_start("pub(in crate::x) use a::b;"), Some(21));
        assert_eq!(use_path_start("user.name = 1;"), None);
        assert_eq!(use_path_start("pub fn used() {}"), None);
        assert_eq!(use_path_start("public use"), None);
    }

    #[test]
    fn prod_code_toml_maps_to_cargo_config() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(ProdCodeConfig::load(temp.path()), ProdCodeConfig::default());
        let defaults = ProdCodeConfig::default().cargo_config();
        assert!(defaults.all_targets);
        assert_eq!(defaults.sysroot, Some(RustLibSource::Discover));
        std::fs::write(
            temp.path().join("prod-code.toml"),
            "[rust]\nfeatures = \"all\"\nsysroot = false\n",
        )
        .unwrap();
        let cfg = ProdCodeConfig::load(temp.path()).cargo_config();
        assert_eq!(cfg.features, CargoFeatures::All);
        assert_eq!(cfg.sysroot, None);
        std::fs::write(
            temp.path().join("prod-code.toml"),
            "[rust]\nfeatures = [\"a\", \"b\"]\nno_default_features = true\n",
        )
        .unwrap();
        let cfg = ProdCodeConfig::load(temp.path()).cargo_config();
        assert_eq!(
            cfg.features,
            CargoFeatures::Selected {
                features: vec!["a".into(), "b".into()],
                no_default_features: true
            }
        );
    }

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
    fn test_session_overlays_do_not_bleed_between_sessions() {
        let (temp, lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
        let base = std::fs::read_to_string(&lib_path).unwrap();
        let text_a = format!("{base}pub const ONLY_IN_SESSION_A: u8 = 1;\n");
        let text_b = format!("{base}pub const ONLY_IN_SESSION_B: u8 = 2;\n");

        engine
            .set_session_overlay(1, &lib_path, Some(text_a))
            .unwrap();
        engine
            .set_session_overlay(2, &lib_path, Some(text_b))
            .unwrap();

        let names = |engine: &RustEngine| -> Vec<String> {
            engine
                .document_symbols(&lib_path)
                .unwrap()
                .into_iter()
                .map(|s| s.name)
                .collect()
        };

        engine.activate_session(1).unwrap();
        let a = names(&engine);
        assert!(a.contains(&"ONLY_IN_SESSION_A".to_string()), "{a:?}");
        assert!(!a.contains(&"ONLY_IN_SESSION_B".to_string()), "{a:?}");

        assert_eq!(engine.activate_session(2).unwrap(), 1);
        let b = names(&engine);
        assert!(b.contains(&"ONLY_IN_SESSION_B".to_string()), "{b:?}");
        assert!(!b.contains(&"ONLY_IN_SESSION_A".to_string()), "{b:?}");

        // A session without its own buffer sees the untouched base.
        assert_eq!(engine.activate_session(3).unwrap(), 1);
        let plain = names(&engine);
        assert!(plain.contains(&"DEFAULT_PORT".to_string()));
        assert!(!plain.iter().any(|n| n.starts_with("ONLY_IN_SESSION")));
        assert_eq!(engine.activate_session(3).unwrap(), 0);

        // Closing the buffer returns that session to the base as well.
        engine.clear_session(1).unwrap();
        engine.activate_session(1).unwrap();
        let after_close = names(&engine);
        assert!(!after_close.contains(&"ONLY_IN_SESSION_A".to_string()));
        assert_eq!(engine.session_overlay_count(1), 0);
        assert_eq!(engine.session_overlay_count(2), 1);
    }

    #[test]
    fn test_session_overlay_untracked_file_is_private() {
        let (temp, _lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
        let scratch = temp.path().join("src/scratch.rs");
        engine
            .set_session_overlay(7, &scratch, Some("pub fn scratch_only() {}\n".to_string()))
            .unwrap();

        engine.activate_session(7).unwrap();
        let mine = engine.document_symbols(&scratch).unwrap();
        assert!(mine.iter().any(|s| s.name == "scratch_only"));

        engine.activate_session(8).unwrap();
        let theirs = engine.document_symbols(&scratch).unwrap_or_default();
        assert!(theirs.is_empty(), "{theirs:?}");

        engine.activate_session(7).unwrap();
        let again = engine.document_symbols(&scratch).unwrap();
        assert!(again.iter().any(|s| s.name == "scratch_only"));
    }

    #[test]
    fn test_rename_rewrites_definition_and_uses() {
        let (temp, lib_path) = create_test_fixture();
        let engine = RustEngine::load(temp.path()).expect("Must load fixture");
        // `pub struct PathTranslator {` is line 3; the name starts at column 12.
        let outcome = engine
            .rename(&lib_path, 3, 12, "PathMapper")
            .expect("rename query")
            .expect("rename accepted");
        assert_eq!(outcome.files.len(), 1);
        let file = &outcome.files[0];
        assert!(
            file.new_text.contains("pub struct PathMapper {"),
            "{}",
            file.new_text
        );
        assert!(
            file.new_text.contains("impl PathMapper {"),
            "{}",
            file.new_text
        );
        assert!(!file.new_text.contains("PathTranslator"));
        assert_eq!(file.edits, 2);
        assert!(file.old_line_count >= 10);
        assert!(outcome.moves.is_empty());

        // Not a symbol: rust-analyzer refuses instead of the engine erroring.
        let refused = engine.rename(&lib_path, 1, 1, "x").expect("rename query");
        assert!(refused.is_err());
    }

    #[test]
    fn test_safe_delete_refuses_used_and_removes_unused() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        let lib = temp.path().join("src/lib.rs");
        std::fs::write(
            &lib,
            "pub fn used() -> u8 {\n    1\n}\n\npub fn unused() -> u8 {\n    2\n}\n\npub fn caller() -> u8 {\n    used()\n}\n",
        )
        .unwrap();
        let engine = RustEngine::load(temp.path()).expect("Must load fixture");

        let refused = engine.safe_delete(&lib, 1, 8).unwrap().unwrap_err();
        assert!(refused.contains("1 usage(s)"), "{refused}");
        assert!(refused.contains("lib.rs:10:"), "{refused}");

        // Inside a body, on no item's name: refused, and nothing of the body goes (#138).
        let nothing = engine.safe_delete(&lib, 6, 5).unwrap().unwrap_err();
        assert!(nothing.contains("no deletable item is named"), "{nothing}");

        let outcome = engine
            .safe_delete(&lib, 5, 8)
            .unwrap()
            .expect("unused item deletes");
        let text = &outcome.files[0].new_text;
        assert!(!text.contains("unused"), "{text}");
        assert!(text.contains("pub fn used()") && text.contains("pub fn caller()"));
        assert!(!text.contains("\n\n\n"), "{text:?}");
    }

    #[test]
    fn test_assists_list_and_apply_inline_variable() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        let lib = temp.path().join("src/lib.rs");
        std::fs::write(
            &lib,
            "pub fn f() -> i32 {\n    let value = 40 + 2;\n    value\n}\n",
        )
        .unwrap();
        let engine = RustEngine::load(temp.path()).expect("Must load fixture");

        // Cursor on `value` in `let value = ...` (line 2, col 9).
        let offered = engine.list_assists(&lib, 2, 9, None).unwrap();
        assert!(
            offered.iter().any(|a| a.id == "inline_local_variable"),
            "{offered:?}"
        );

        let outcome = engine
            .apply_assist(&lib, 2, 9, None, "inline_local_variable", None)
            .unwrap()
            .expect("assist applies");
        assert_eq!(outcome.files.len(), 1);
        let text = &outcome.files[0].new_text;
        assert!(!text.contains("let value"), "{text}");
        assert!(text.contains("40 + 2"), "{text}");

        let refused = engine
            .apply_assist(&lib, 2, 9, None, "no_such_assist", None)
            .unwrap();
        let reason = refused.expect_err("refused");
        assert!(reason.contains("not offered here; available"), "{reason}");
        assert!(!reason.contains("macro"), "not inside a macro: {reason}");
    }

    /// Inside a macro call's input most refactorings are not offered; the refusal says which
    /// macro and what to do, rather than only "not offered here" (#99).
    #[test]
    fn an_assist_refused_inside_a_macro_call_names_the_macro() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        let lib = temp.path().join("src/lib.rs");
        std::fs::write(
            &lib,
            "macro_rules! wrap {\n    ($($t:tt)*) => { $($t)* };\n}\n\npub fn g() -> i32 {\n    wrap! {\n        let y = 1 + 2;\n    }\n    0\n}\n",
        )
        .unwrap();
        let engine = RustEngine::load(temp.path()).expect("Must load fixture");
        let refused = engine
            .apply_assist(&lib, 7, 17, Some((7, 22)), "extract_function", None)
            .unwrap();
        let reason = refused.expect_err("nothing is extracted inside a macro call");
        assert!(reason.contains("inside `wrap!`"), "{reason}");
        assert!(
            reason.contains("Move the code out of the macro"),
            "{reason}"
        );
    }

    #[test]
    fn test_identical_text_does_not_change_revision() {
        let (temp, lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
        let text = std::fs::read_to_string(&lib_path).unwrap();
        engine.apply_file_change(&lib_path, text.clone()).unwrap();
        // A snapshot taken now survives a no-op re-open; a real revision would cancel it.
        let snapshot = engine.snapshot();
        engine.apply_file_change(&lib_path, text.clone()).unwrap();
        assert!(snapshot.hover(&lib_path, 1, 15).unwrap().is_some());
        // Release the snapshot before a real change: apply_change waits for outstanding
        // snapshots, so holding one here would deadlock the test.
        drop(snapshot);
        // A genuine change still lands.
        engine
            .apply_file_change(&lib_path, format!("{text}pub const CHANGED: u8 = 1;\n"))
            .unwrap();
        assert!(
            engine
                .document_symbols(&lib_path)
                .unwrap()
                .iter()
                .any(|s| s.name == "CHANGED")
        );
    }

    #[test]
    fn test_update_base_adds_module_file_missing_at_load() {
        let (temp, lib_path) = create_test_fixture();
        // The crate declares a module whose file does not exist yet when the engine loads.
        let mut lib = std::fs::read_to_string(&lib_path).unwrap();
        lib.push_str("\nmod late_module;\n");
        std::fs::write(&lib_path, lib).unwrap();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");

        let late = temp.path().join("src/late_module.rs");
        std::fs::write(&late, "pub fn late_fn() -> u8 {{ 3 }}\n").unwrap();
        engine
            .update_base(&late, Some("pub fn late_fn() -> u8 {{ 3 }}\n".to_string()))
            .unwrap();
        let hover = engine.hover(&late, 1, 8).unwrap();
        assert!(
            hover.as_deref().is_some_and(|h| h.contains("late_fn")),
            "{hover:?}"
        );
    }

    #[test]
    fn test_update_base_respects_open_buffers() {
        let (temp, lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
        let names = |engine: &RustEngine| -> Vec<String> {
            engine
                .document_symbols(&lib_path)
                .unwrap()
                .into_iter()
                .map(|s| s.name)
                .collect()
        };

        engine
            .update_base(
                &lib_path,
                Some("pub const FROM_SYNC: u8 = 1;\n".to_string()),
            )
            .unwrap();
        assert!(names(&engine).contains(&"FROM_SYNC".to_string()));

        engine
            .set_session_overlay(
                9,
                &lib_path,
                Some("pub const FROM_BUFFER: u8 = 2;\n".to_string()),
            )
            .unwrap();
        engine
            .update_base(
                &lib_path,
                Some("pub const FROM_SYNC_V2: u8 = 3;\n".to_string()),
            )
            .unwrap();
        engine.activate_session(9).unwrap();
        let with_buffer = names(&engine);
        assert!(
            with_buffer.contains(&"FROM_BUFFER".to_string()),
            "{with_buffer:?}"
        );
        assert!(
            !with_buffer.contains(&"FROM_SYNC_V2".to_string()),
            "{with_buffer:?}"
        );

        engine.clear_session(9).unwrap();
        let after = names(&engine);
        assert!(after.contains(&"FROM_SYNC_V2".to_string()), "{after:?}");
    }

    #[test]
    fn test_retain_session_overlays_drops_stale_buffers() {
        let (temp, lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
        let scratch = temp.path().join("src/scratch.rs");
        engine
            .set_session_overlay(5, &lib_path, Some("pub const STALE: u8 = 1;\n".to_string()))
            .unwrap();
        engine
            .set_session_overlay(5, &scratch, Some("pub fn keep_me() {}\n".to_string()))
            .unwrap();
        assert_eq!(
            engine
                .retain_session_overlays(5, std::slice::from_ref(&scratch))
                .unwrap(),
            1
        );
        assert_eq!(engine.session_overlay_count(5), 1);
        engine.activate_session(5).unwrap();
        let names: Vec<String> = engine
            .document_symbols(&lib_path)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(names.contains(&"DEFAULT_PORT".to_string()), "{names:?}");
        assert!(!names.contains(&"STALE".to_string()), "{names:?}");
    }

    #[test]
    fn test_session_overlay_new_module_survives_view_switches() {
        let (temp, lib_path) = create_test_fixture();
        let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
        let scratch = temp.path().join("src/scratch.rs");
        let base = std::fs::read_to_string(&lib_path).unwrap();
        let owner = format!("{base}\n#[path = \"scratch.rs\"]\nmod scratch;\n");

        engine
            .set_session_overlay(1, &lib_path, Some(owner))
            .unwrap();
        engine
            .set_session_overlay(
                1,
                &scratch,
                Some("pub fn scratch_only() -> u8 {{ 7 }}\n".to_string()),
            )
            .unwrap();
        engine.activate_session(1).unwrap();
        let first = engine.hover(&scratch, 1, 8).unwrap();
        assert!(
            first.as_deref().is_some_and(|h| h.contains("scratch_only")),
            "{first:?}"
        );

        // Another session looks at the base, then session 1 comes back.
        engine.activate_session(2).unwrap();
        assert!(engine.hover(&scratch, 1, 8).unwrap().is_none());
        engine.activate_session(1).unwrap();
        let again = engine.hover(&scratch, 1, 8).unwrap();
        assert!(
            again.as_deref().is_some_and(|h| h.contains("scratch_only")),
            "{again:?}"
        );

        // Session 1 disconnects; a fresh session re-syncs the same files and must resolve too.
        engine.clear_session(1).unwrap();
        engine.activate_session(2).unwrap();
        let owner = format!("{base}\n#[path = \"scratch.rs\"]\nmod scratch;\n");
        engine
            .set_session_overlay(3, &lib_path, Some(owner))
            .unwrap();
        engine
            .set_session_overlay(
                3,
                &scratch,
                Some("pub fn scratch_only() -> u8 {{ 7 }}\n".to_string()),
            )
            .unwrap();
        engine.activate_session(3).unwrap();
        let fresh = engine.hover(&scratch, 1, 8).unwrap();
        assert!(
            fresh.as_deref().is_some_and(|h| h.contains("scratch_only")),
            "{fresh:?}"
        );
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
