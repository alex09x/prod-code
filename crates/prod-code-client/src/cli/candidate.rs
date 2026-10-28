/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::commands::Commands;
use std::path::{Path, PathBuf};

pub fn command_path_tokens(command: Option<&Commands>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Some(cmd) = command else {
        return paths;
    };
    match cmd {
        Commands::Def { file: Some(f), .. }
        | Commands::Hover { file: Some(f), .. }
        | Commands::Refs { file: Some(f), .. }
        | Commands::Callers { file: Some(f), .. }
        | Commands::Callees { file: Some(f), .. }
        | Commands::Impls { file: Some(f), .. }
        | Commands::Supertypes { file: Some(f), .. }
        | Commands::SafeDelete { file: f, .. }
        | Commands::Rename { file: f, .. }
        | Commands::Assists { file: f, .. }
        | Commands::Assist { file: f, .. }
        | Commands::Diagnostics { file: f, .. }
        | Commands::Outline { file: f, .. }
        | Commands::InlineParameter { file: f, .. }
        | Commands::ExtractDelegate { file: f, .. }
        | Commands::ExtractTrait { file: f, .. }
        | Commands::LoopToIterator { file: f, .. }
        | Commands::ExtractFunction { file: f, .. }
        | Commands::IntroduceVariable { file: f, .. }
        | Commands::ReplaceConstructorWithFactory { file: f, .. }
        | Commands::ReplaceConstructorWithBuilder { file: f, .. }
        | Commands::PullUp { file: f, .. }
        | Commands::PushDown { file: f, .. }
        | Commands::ReplaceInheritanceWithDelegation { file: f, .. }
        | Commands::ReplaceConditionalWithPolymorphism { file: f, .. }
        | Commands::ExtractInterface { file: f, .. }
        | Commands::ExtractParameter { file: f, .. }
        | Commands::MoveMethod { file: f, .. }
        | Commands::ProposeExpression { file: f, .. } => {
            paths.push(f.clone());
        }
        Commands::MoveModule { file, to, .. } => {
            paths.push(file.clone());
            paths.push(to.clone());
        }
        Commands::Move { to, path, .. } => {
            paths.push(PathBuf::from(to));
            if let Some(p) = path {
                paths.push(PathBuf::from(p));
            }
        }
        Commands::MakeStatic { path, symbol, .. }
        | Commands::WrapReturn { path, symbol, .. }
        | Commands::ParameterObject { path, symbol, .. }
        | Commands::MigrateType { path, symbol, .. }
        | Commands::Generify { path, symbol, .. }
        | Commands::InvertBoolean { path, symbol, .. }
        | Commands::ConvertToMethod { path, symbol, .. }
        | Commands::EncapsulateField { path, symbol, .. } => {
            if let Some(p) = path {
                paths.push(PathBuf::from(p));
            }
            paths.push(PathBuf::from(symbol));
        }
        Commands::SchemaRename { path, .. }
        | Commands::Codemod { path, .. }
        | Commands::Search { path, .. }
        | Commands::ChangeSignature { path, .. }
        | Commands::Fixture { path, .. } => {
            if let Some(p) = path {
                paths.push(PathBuf::from(p));
            }
        }
        Commands::Grep {
            target_path, path, ..
        } => {
            if let Some(p) = path.as_ref().or(target_path.as_ref()) {
                paths.push(PathBuf::from(p));
            }
        }
        Commands::Symbols { target } | Commands::Slice { target, .. } => {
            paths.push(PathBuf::from(target));
        }
        Commands::Source { path, .. } => {
            paths.push(PathBuf::from(path));
        }
        Commands::Sync { path: Some(p), .. }
        | Commands::Check { path: Some(p), .. }
        | Commands::Lint { path: Some(p), .. }
        | Commands::Test { path: Some(p), .. }
        | Commands::Benchmarks { path: Some(p), .. }
        | Commands::Dependencies { path: Some(p), .. }
        | Commands::Duplicates { path: Some(p), .. }
        | Commands::StructuralSearch { path: Some(p), .. } => {
            paths.push(p.clone());
        }
        Commands::Validate {
            file,
            from,
            diff,
            with,
            ..
        } => {
            if let Some(f) = file {
                paths.push(f.clone());
            }
            if let Some(fr) = from {
                paths.push(fr.clone());
            }
            if let Some(d) = diff {
                paths.push(d.clone());
            }
            for w in with {
                if let Some((target, replacement)) = w.split_once('=') {
                    paths.push(PathBuf::from(target));
                    paths.push(PathBuf::from(replacement));
                } else {
                    paths.push(PathBuf::from(w));
                }
            }
        }
        _ => {}
    }
    paths
}

/// Where a command's placement is judged: the first file it names, else the first directory,
/// else the current directory, all inside `root`; `root` when none is. A relative path is the
/// one the command itself will use, from `cwd` (`check --path nested` in a subdirectory), and
/// only else from `root`, so that the node chosen at startup serves the project the command
/// runs (#1478).
pub fn placement_hint(root: &Path, cwd: &Path, tokens: &[PathBuf]) -> PathBuf {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let resolved = |p: &PathBuf| -> Vec<PathBuf> {
        if p.is_absolute() {
            vec![p.clone()]
        } else {
            vec![cwd.join(p), root.join(p)]
        }
    };
    let inside = |candidate: &Path| {
        std::fs::canonicalize(candidate)
            .ok()
            .filter(|c| c.starts_with(&canonical_root))
    };
    let file = tokens.iter().find_map(|p| {
        resolved(p)
            .into_iter()
            .find(|c| c.is_file())
            .and_then(|c| inside(&c))
    });
    let dir = || {
        tokens.iter().find_map(|p| {
            resolved(p)
                .into_iter()
                .find(|c| c.is_dir())
                .and_then(|c| inside(&c))
                .filter(|c| *c != canonical_root)
        })
    };
    file.or_else(dir)
        .or_else(|| inside(cwd))
        .unwrap_or(canonical_root)
}

#[cfg(test)]
mod tests {
    use super::{command_path_tokens, placement_hint};
    use crate::cli::Commands;
    use std::path::PathBuf;

    #[test]
    fn exec_and_pull_do_not_extract_language_hint_paths() {
        let exec_cmd = Commands::Exec {
            timeout_secs: 0,
            no_pull: false,
            env: vec![],
            command: vec![
                "python3".to_string(),
                "scripts/cluster-rollout.py".to_string(),
                "--status".to_string(),
            ],
        };
        assert!(
            command_path_tokens(Some(&exec_cmd)).is_empty(),
            "exec command arguments must not force a script language engine (#1088)"
        );

        let pull_cmd = Commands::Pull {
            files: vec![PathBuf::from("target/release/prod-code")],
            list: false,
        };
        assert!(
            command_path_tokens(Some(&pull_cmd)).is_empty(),
            "pull paths must use the base workspace placement (#1087)"
        );
    }

    /// #1478: run from a subdirectory, `check --path nested` names the project under that
    /// subdirectory, as the command itself resolves it, not one under the checkout root.
    #[test]
    fn a_relative_path_is_placed_from_where_the_command_runs() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::create_dir_all(root.join("tools/nested")).unwrap();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("tools/main.rs"), "").unwrap();
        let cwd = root.join("tools");
        let nested = vec![PathBuf::from("nested")];
        assert_eq!(
            placement_hint(&root, &cwd, &nested),
            root.join("tools/nested")
        );
        // From the root itself the same token is the root's `nested`.
        assert_eq!(placement_hint(&root, &root, &nested), root.join("nested"));
        // A file named relative to the root, which the subdirectory does not have, still counts.
        let file = vec![PathBuf::from("tools/main.rs")];
        assert_eq!(
            placement_hint(&root, &cwd, &file),
            root.join("tools/main.rs")
        );
        // Nothing named: the current directory; outside the checkout: the root.
        assert_eq!(placement_hint(&root, &cwd, &[]), cwd);
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(placement_hint(&root, outside.path(), &[]), root);
        let elsewhere = vec![outside.path().to_path_buf()];
        assert_eq!(placement_hint(&root, &cwd, &elsewhere), cwd);
    }
}
