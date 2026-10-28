/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::render::shell_words;
use super::test_cmd::test_command;
use super::types::{ImpactTestPlan, Symbol};
use crate::verify::{ProjectTools, VerifyKind, detect_tools, plan_command_with};

type Scope = (String, String);

fn configuration_scope(root: &Path, file: &str, default_language: &str) -> Scope {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = root.join(file);
    let parent = path.parent().unwrap_or(&root);
    let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    if !parent.starts_with(&root) {
        return (file.to_string(), "unknown".into());
    }
    let root_engine = crate::sync::engine::detect::engine_at(&root);
    for dir in parent
        .ancestors()
        .take_while(|dir| dir.starts_with(&root) && *dir != root)
    {
        if crate::sync::is_in_dependency_dir(dir) {
            continue;
        }
        if let Some(engine) = crate::sync::manifest_engine_at(dir) {
            if engine == "rust"
                && Some(engine) == root_engine
                && !crate::sync::engine::workspace::excluded_from_root_workspace(&root, dir)
            {
                break;
            }
            if let Ok(relative) = dir.strip_prefix(&root) {
                return (
                    relative.to_string_lossy().replace('\\', "/"),
                    engine.to_string(),
                );
            }
        }
    }
    (String::new(), default_language.to_string())
}

fn scope(root: &Path, file: &str, default_language: &str) -> Scope {
    let path = Path::new(file);
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return (file.to_string(), "unknown".into());
    }
    if !super::symbols::is_source_file(file) {
        return configuration_scope(root, file, default_language);
    }
    let abs = root.join(file);
    let configured = configuration_scope(root, file, default_language);
    if !configured.0.is_empty() && crate::sync::engine_for_file(&abs) == Some(configured.1.as_str())
    {
        return configured;
    }
    let (subpath, mut language) = crate::sync::engine_project(root, &abs);
    let root_language = crate::sync::expected_engine(root);
    if let Some(own) = crate::sync::engine_for_file(&abs)
        && language.as_deref() == root_language.as_deref()
        && Some(own) != language.as_deref()
        && !matches!(own, "markdown" | "yaml" | "toml" | "json" | "html" | "css")
    {
        language = Some(own);
    }
    (
        subpath.unwrap_or_default(),
        language.unwrap_or(default_language).to_string(),
    )
}

fn label((path, language): &Scope) -> String {
    format!("{} ({language})", if path.is_empty() { "." } else { path })
}

fn compose(commands: Vec<(String, Vec<String>)>) -> Option<Vec<String>> {
    if commands.is_empty() {
        return None;
    }
    if commands.len() == 1 && commands[0].0.is_empty() {
        return commands.into_iter().next().map(|(_, command)| command);
    }
    let steps = commands
        .into_iter()
        .map(|(path, command)| {
            let dir = if path.is_empty() {
                ".".to_string()
            } else {
                path
            };
            format!("(cd {} && {})", shell_words(&[dir]), shell_words(&command))
        })
        .collect::<Vec<_>>();
    Some(vec![
        "sh".into(),
        "-c".into(),
        steps.join(" && "),
        "prod-code-impact".into(),
    ])
}

pub(super) fn build_test_plan(
    root: &Path,
    default_language: &str,
    root_tools: &ProjectTools,
    tests: &[Symbol],
    changed_files: &[String],
) -> ImpactTestPlan {
    let mut groups: BTreeMap<Scope, Vec<Symbol>> = BTreeMap::new();
    for test in tests {
        let group = scope(root, &test.file, default_language);
        let mut test = test.clone();
        if !group.0.is_empty()
            && let Ok(relative) = Path::new(&test.file).strip_prefix(&group.0)
        {
            test.file = relative.to_string_lossy().into_owned();
        }
        groups.entry(group).or_default().push(test);
    }
    let mut plan = ImpactTestPlan::default();
    let mut selected = Vec::new();
    for (group, tests) in &groups {
        let nested = (!group.0.is_empty()).then(|| detect_tools(&root.join(&group.0)));
        let tools = nested.as_ref().unwrap_or(root_tools);
        let fallback = plan_command_with(tools, &group.1, VerifyKind::Test, None).ok();
        let command = match test_command(&group.1, tools, tests) {
            Some(command) => {
                if fallback.as_ref() == Some(&command) {
                    plan.selection_fallbacks.push(label(group));
                }
                Some(command)
            }
            None => {
                if fallback.is_some() {
                    plan.selection_fallbacks.push(label(group));
                }
                fallback
            }
        };
        match command {
            Some(command) => selected.push((group.0.clone(), command)),
            None => plan.unsupported_selected.push(label(group)),
        }
    }
    if plan.unsupported_selected.is_empty() {
        plan.selected = compose(selected);
    }

    let mut scopes: BTreeSet<Scope> = groups.keys().cloned().collect();
    scopes.extend(
        changed_files
            .iter()
            .map(|file| scope(root, file, default_language)),
    );
    if scopes.is_empty() {
        scopes.insert((String::new(), default_language.to_string()));
    }
    plan.whole_is_root_only = scopes.len() == 1
        && scopes
            .iter()
            .next()
            .is_some_and(|(path, language)| path.is_empty() && language == default_language);
    let mut whole = Vec::new();
    for group in scopes {
        let nested = (!group.0.is_empty()).then(|| detect_tools(&root.join(&group.0)));
        let tools = nested.as_ref().unwrap_or(root_tools);
        match plan_command_with(tools, &group.1, VerifyKind::Test, None) {
            Ok(command) => whole.push((group.0.clone(), command)),
            Err(_) => plan.unsupported_whole.push(label(&group)),
        }
    }
    if plan.unsupported_whole.is_empty() {
        plan.whole = compose(whole);
    }
    plan
}
