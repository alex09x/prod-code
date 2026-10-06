/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::target::{discover_target, go_package_name, mutate_signature, run_git};
use super::types::{
    BENCH_WORKSPACE_SUFFIX, DivergenceSetup, DivergentTarget, DivergentWorktree,
    FIXTURE_CARGO_TOML, FIXTURE_LIB_RS, FIXTURE_REPO_NAME, Language, WorkspaceMode, WorktreeKind,
};
use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};

fn write_fixture_crate(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("Cargo.toml"), FIXTURE_CARGO_TOML)?;
    std::fs::write(root.join("src/lib.rs"), FIXTURE_LIB_RS)?;
    Ok(())
}

/// Name of the origin clone and of the shared server workspace for `base_repo`.
pub fn bench_workspace_name(base_repo: Option<&Path>) -> String {
    let repo_name = base_repo
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(FIXTURE_REPO_NAME);
    format!("{repo_name}{BENCH_WORKSPACE_SUFFIX}")
}

/// Prepares the origin repository: clones `base_repo` locally (leaving the original untouched)
/// or initializes a disposable scratch repo with a root fixture crate. The clone directory is
/// named after the benchmark workspace so worktrees created from it resolve to that name.
fn prepare_origin(base_repo: Option<&Path>, workdir: &Path, name: &str) -> Result<PathBuf> {
    let origin = workdir.join(name);
    if origin.exists() {
        bail!("origin path {origin:?} already exists; use a fresh workdir");
    }
    match base_repo {
        Some(src) => {
            let src_str = src
                .to_str()
                .ok_or_else(|| anyhow!("base repo path must be valid UTF-8: {:?}", src))?;
            let origin_str = origin
                .to_str()
                .ok_or_else(|| anyhow!("workdir path must be valid UTF-8: {:?}", origin))?;
            run_git(
                workdir,
                &[
                    "clone",
                    "--quiet",
                    "--local",
                    "--no-hardlinks",
                    src_str,
                    origin_str,
                ],
            )?;
        }
        None => {
            std::fs::create_dir_all(&origin)?;
            run_git(&origin, &["init", "--quiet", "--initial-branch=main"])?;
            run_git(
                &origin,
                &["config", "user.email", "divergent-bench@prod.codes"],
            )?;
            run_git(
                &origin,
                &["config", "user.name", "prod-code divergent-bench"],
            )?;
            write_fixture_crate(&origin)?;
            run_git(&origin, &["add", "-A"])?;
            run_git(
                &origin,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    "divergent-bench: seed fixture crate",
                ],
            )?;
        }
    }
    Ok(origin)
}

/// Detects the base repository language from its root manifest.
pub fn detect_language(root: &Path) -> Result<Language> {
    if root.join(Language::Rust.manifest()).exists() {
        Ok(Language::Rust)
    } else if root.join(Language::Go.manifest()).exists() {
        Ok(Language::Go)
    } else {
        bail!(
            "no Cargo.toml or go.mod at {root:?}; the divergent benchmark needs a Rust or Go repository root"
        )
    }
}

/// The directory of copy `copy` (from 0) of a worktree of `kind`: `wt-signature`,
/// `wt-signature-2`, ...
fn copy_dir_name(kind: WorktreeKind, copy: usize) -> String {
    match copy {
        0 => kind.dir_name().to_string(),
        n => format!("{}-{}", kind.dir_name(), n + 1),
    }
}

fn create_worktree(
    origin: &Path,
    workdir: &Path,
    kind: WorktreeKind,
    copy: usize,
) -> Result<PathBuf> {
    let dir_name = copy_dir_name(kind, copy);
    let path = workdir.join(&dir_name);
    let branch = format!("divergent-bench-{dir_name}");
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow!("worktree path must be valid UTF-8: {:?}", path))?;
    run_git(
        origin,
        &[
            "worktree", "add", "--quiet", "-b", &branch, path_str, "HEAD",
        ],
    )?;
    Ok(path)
}

/// Applies the controlled mutation for `kind` inside `root`, returning the file the benchmark
/// should query against this worktree and the symbol to hover.
fn apply_mutation(
    root: &Path,
    kind: WorktreeKind,
    target: &DivergentTarget,
) -> Result<(PathBuf, String)> {
    let target_file = root.join(&target.file_rel);
    let language = target.language;

    match kind {
        WorktreeKind::Master => Ok((target_file, target.symbol.clone())),
        WorktreeKind::SignatureChange => {
            let content = std::fs::read_to_string(&target_file)?;
            let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
            let line = lines
                .get_mut(target.line)
                .ok_or_else(|| anyhow!("target line {} out of range", target.line))?;
            *line = mutate_signature(line, language)?;
            let mut rewritten = lines.join("\n");
            if content.ends_with('\n') {
                rewritten.push('\n');
            }
            std::fs::write(&target_file, rewritten)?;
            Ok((target_file, target.symbol.clone()))
        }
        WorktreeKind::DependencyChange => {
            let manifest = root.join(language.manifest());
            let mut content = std::fs::read_to_string(&manifest)?;
            content.push_str(language.manifest_touch_line());
            std::fs::write(&manifest, content)?;
            Ok((target_file, target.symbol.clone()))
        }
        WorktreeKind::UntrackedFile => {
            let dir = target_file
                .parent()
                .ok_or_else(|| anyhow!("target file has no parent: {target_file:?}"))?;
            let untracked = dir.join(language.untracked_file_name());
            let symbol = language.untracked_symbol();
            let body = match language {
                Language::Rust => {
                    // A new Rust file is only analyzable once the crate declares it as a module,
                    // which is what an agent does right after creating a scratch file.
                    let mut owner = std::fs::read_to_string(&target_file)?;
                    if !owner.ends_with('\n') {
                        owner.push('\n');
                    }
                    owner.push_str(
                        "\n#[path = \"divergent_untracked.rs\"]\nmod divergent_untracked;\n",
                    );
                    std::fs::write(&target_file, owner)?;
                    format!(
                        "pub fn {symbol}() -> &'static str {{\n    \"divergent-bench-marker\"\n}}\n"
                    )
                }
                Language::Go => {
                    let package = go_package_name(&std::fs::read_to_string(&target_file)?)
                        .ok_or_else(|| anyhow!("no package clause in {target_file:?}"))?;
                    format!(
                        "package {package}\n\n// {symbol} is introduced by the divergent benchmark.\nfunc {symbol}() string {{\n\treturn \"divergent-bench-marker\"\n}}\n"
                    )
                }
            };
            std::fs::write(&untracked, body)?;
            Ok((untracked, symbol.to_string()))
        }
    }
}

/// Creates the origin repository, discovers the target symbol, and materializes `copies`
/// diverged, mutated worktrees of every kind.
pub fn setup(
    base_repo: Option<&Path>,
    workdir: &Path,
    mode: WorkspaceMode,
    copies: usize,
) -> Result<DivergenceSetup> {
    let workspace_name = bench_workspace_name(base_repo);
    let origin = prepare_origin(base_repo, workdir, &workspace_name)?;
    let language = detect_language(&origin)?;
    let target = discover_target(&origin, language)?;

    let mut worktrees = Vec::with_capacity(4 * copies);
    for copy in 0..copies {
        for kind in WorktreeKind::all() {
            let root = create_worktree(&origin, workdir, kind, copy)?;
            let (query_file, symbol) = apply_mutation(&root, kind, &target)?;
            let wt_workspace_name = match mode {
                WorkspaceMode::Shared => workspace_name.clone(),
                // Same shape as production worktree workspaces (`<repo>--wt-<id>`), so the
                // gateway's seeding, pruning and shared-target logic applies to the bench too.
                WorkspaceMode::Isolated => format!(
                    "{workspace_name}--wt-{}",
                    copy_dir_name(kind, copy).trim_start_matches("wt-")
                ),
            };
            worktrees.push(DivergentWorktree {
                kind,
                root,
                query_file,
                symbol,
                workspace_name: wt_workspace_name,
            });
        }
    }

    Ok(DivergenceSetup {
        origin,
        workspace_name,
        target,
        worktrees,
    })
}
