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
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::command_path_tokens;
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
        };
        assert!(
            command_path_tokens(Some(&pull_cmd)).is_empty(),
            "pull paths must use the base workspace placement (#1087)"
        );
    }
}
