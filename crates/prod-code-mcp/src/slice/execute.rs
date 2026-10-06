/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::candidates::{candidate_names, check_pos, lines_of, relative};
use super::facts::{FileFacts, decl_at, file_facts};
use super::lsp::{Decl, Pos, Source, parse_locations};
use super::types::{GapKind, MAX_NAMES_PER_ITEM, SliceGap, SliceItem, SliceOptions, SliceReport};
use crate::session::LspSession;

/// Builds the slice. `seed` is a file and a 1-based position on the symbol's name.
pub async fn slice(
    remote: SocketAddr,
    root: &Path,
    seed_file: &Path,
    seed_line: u32,
    seed_col: u32,
    depth: u32,
    max_bytes: usize,
) -> Result<SliceReport> {
    slice_with_options(
        remote,
        root,
        seed_file,
        seed_line,
        seed_col,
        SliceOptions {
            depth,
            max_bytes,
            dataflow: false,
            target_line: None,
            target_var: None,
        },
    )
    .await
}

/// Builds the slice with customized slicing options (including intra-function data-flow).
pub async fn slice_with_options(
    remote: SocketAddr,
    root: &Path,
    seed_file: &Path,
    seed_line: u32,
    seed_col: u32,
    options: SliceOptions,
) -> Result<SliceReport> {
    let mut session = LspSession::open(remote, root, Some(seed_file)).await?;
    let result = slice_with(&mut session, root, seed_file, seed_line, seed_col, options).await;
    session.close().await;
    result
}

/// An item waiting to be sliced: its file, its declaration, its depth and who named it.
type Queued = (PathBuf, Decl, u32, Option<String>);

async fn slice_with(
    session: &mut LspSession,
    root: &Path,
    seed_file: &Path,
    seed_line: u32,
    seed_col: u32,
    options: SliceOptions,
) -> Result<SliceReport> {
    let (Some(line), Some(character)) = (seed_line.checked_sub(1), seed_col.checked_sub(1)) else {
        anyhow::bail!(
            "seed position {seed_line}:{seed_col} is not 1-based: line and character start at 1"
        );
    };
    let seed_pos = Pos { line, character };
    let mut facts: BTreeMap<PathBuf, FileFacts> = BTreeMap::new();
    facts.insert(
        seed_file.to_path_buf(),
        file_facts(session, seed_file).await?,
    );
    // A column past the line would still find the declaration spanning the line; a caller that
    // only knows the line gives column 1, which every line holds.
    if let Err(e) = check_pos(&facts[seed_file].text, &facts[seed_file].lines, seed_pos) {
        anyhow::bail!(
            "no declaration at {}:{seed_line}:{seed_col}: the seed {e}",
            relative(root, seed_file)
        );
    }

    let seed_decl = decl_at(&facts[seed_file].decls, seed_pos)
        .cloned()
        .with_context(|| {
            format!(
                "no declaration at {}:{seed_line}",
                relative(root, seed_file)
            )
        })?;

    let mut report = SliceReport {
        seed: seed_decl.name.clone(),
        depth_limit: options.depth,
        max_bytes: options.max_bytes,
        ..Default::default()
    };
    // Files that could not be read or listed, with why, so each is tried once.
    let mut unreadable: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut queued: HashSet<(PathBuf, Pos)> = HashSet::new();
    let mut queue: VecDeque<Queued> = VecDeque::new();
    queued.insert((seed_file.to_path_buf(), seed_decl.range.start));
    queue.push_back((seed_file.to_path_buf(), seed_decl, 0, None));

    // Definition queries asked, and those with a well-formed answer.
    let (mut asked, mut answered) = (0usize, 0usize);
    let mut bytes = 0usize;
    while let Some((file, decl, item_depth, because)) = queue.pop_front() {
        let text = {
            let f = &facts[&file];
            // `file_facts` checked every declaration's range against the file.
            lines_of(&f.text, &f.lines, decl.start_line(), decl.end_line()).with_context(|| {
                format!(
                    "`{}` at {}:{}-{} is outside its file",
                    decl.name,
                    relative(root, &file),
                    decl.start_line(),
                    decl.end_line()
                )
            })?
        };
        let rel = relative(root, &file);

        let (display_text, names) = if options.dataflow && item_depth == 0 {
            let df = crate::dataflow::slice_intra_function(
                &facts[&file].text,
                decl.start_line(),
                decl.end_line(),
                &decl.name,
                &rel,
                options.target_line,
                options.target_var.as_deref(),
            );
            let formatted = df.formatted_slice.clone();
            let mut names = Vec::new();
            let mut seen_names = HashSet::new();
            for statement in &df.statements {
                if let Some(source_line) = lines_of(
                    &facts[&file].text,
                    &facts[&file].lines,
                    statement.line,
                    statement.line,
                ) {
                    for candidate in candidate_names(&source_line, statement.line) {
                        if seen_names.insert(candidate.0.clone()) {
                            names.push(candidate);
                        }
                    }
                }
            }
            report.dataflow_slice = Some(df);
            (formatted, names)
        } else {
            let names = candidate_names(&text, decl.start_line());
            (text.clone(), names)
        };

        if bytes.saturating_add(display_text.len()) > options.max_bytes {
            if report.items.is_empty() {
                report.seed_over_budget = true;
            } else {
                report.truncated = 1 + queue.len();
                break;
            }
        }
        bytes = bytes.saturating_add(display_text.len());
        report.items.push(SliceItem {
            file: rel.clone(),
            name: decl.name.clone(),
            kind: decl.kind,
            start_line: decl.start_line(),
            end_line: decl.end_line(),
            depth: item_depth,
            because,
            text: display_text,
        });
        if item_depth >= options.depth {
            report.unexpanded += 1;
            continue;
        }

        if names.len() > MAX_NAMES_PER_ITEM {
            report.gaps.push(SliceGap {
                kind: GapKind::NameLimit,
                item: decl.name.clone(),
                detail: format!(
                    "its body names {} distinct names; only the first {MAX_NAMES_PER_ITEM} were \
                     resolved, and the other {} (from `{}` on) were not looked up",
                    names.len(),
                    names.len() - MAX_NAMES_PER_ITEM,
                    names[MAX_NAMES_PER_ITEM].0
                ),
            });
        }
        let uri = session.uri_for(&file)?;
        for (name, line, col) in names.into_iter().take(MAX_NAMES_PER_ITEM) {
            let at = format!("`{name}` at {rel}:{line}:{col}");
            // Candidates are 1-based by construction.
            let params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            });
            asked += 1;
            let found = match session
                .query(&file, "textDocument/definition", params)
                .await
            {
                Ok(found) => found,
                Err(e) => {
                    report.gaps.push(SliceGap {
                        kind: GapKind::QueryFailed,
                        item: decl.name.clone(),
                        detail: format!("{at}: {e:#}"),
                    });
                    continue;
                }
            };
            let targets = match parse_locations(&found) {
                Ok(targets) => targets,
                Err(e) => {
                    report.gaps.push(SliceGap {
                        kind: GapKind::Malformed,
                        item: decl.name.clone(),
                        detail: format!("{at}: {e}"),
                    });
                    continue;
                }
            };
            // An answer is usable when it is empty or at least one of its targets is not
            // malformed evidence.
            let mut usable = targets.is_empty();
            for target in targets {
                let path = match target.source {
                    Source::File(path) => path,
                    Source::Other { scheme } => {
                        usable = true;
                        report.unsupported.push(format!("{name} ({scheme}:)"));
                        continue;
                    }
                };
                if crate::remote_fs::is_external(root, &path.to_string_lossy()) {
                    usable = true;
                    report.external.push(name.clone());
                    continue;
                }
                // Coordinates are below `u32::MAX`, so the 1-based line fits.
                let there = format!("{}:{}", relative(root, &path), target.range.start.line + 1);
                if !facts.contains_key(&path) {
                    let why = match unreadable.get(&path) {
                        Some(why) => Some(why.clone()),
                        None => match file_facts(session, &path).await {
                            Ok(f) => {
                                facts.insert(path.clone(), f);
                                None
                            }
                            Err(e) => {
                                let why = format!("{e:#}");
                                unreadable.insert(path.clone(), why.clone());
                                Some(why)
                            }
                        },
                    };
                    if let Some(why) = why {
                        usable = true;
                        report.gaps.push(SliceGap {
                            kind: GapKind::Unreadable,
                            item: decl.name.clone(),
                            detail: format!("{at} resolves to {there}: {why}"),
                        });
                        continue;
                    }
                }
                if let Err(e) = facts[&path].check(target.range) {
                    report.gaps.push(SliceGap {
                        kind: GapKind::Malformed,
                        item: decl.name.clone(),
                        detail: format!("{at} resolves to {there}, but its {e}"),
                    });
                    continue;
                }
                usable = true;
                let Some(target_decl) = decl_at(&facts[&path].decls, target.range.start).cloned()
                else {
                    report.unsliced.push(format!("{name} ({there})"));
                    continue;
                };
                // A name that resolves inside the item we are already looking at is a local.
                if path == file && target_decl.range == decl.range {
                    continue;
                }
                if queued.insert((path.clone(), target_decl.range.start)) {
                    queue.push_back((path, target_decl, item_depth + 1, Some(decl.name.clone())));
                }
            }
            if usable {
                answered += 1;
            }
        }
    }
    if asked > 0 && answered == 0 {
        let first = report
            .gaps
            .iter()
            .find(|g| matches!(g.kind, GapKind::QueryFailed | GapKind::Malformed))
            .map(|g| g.detail.clone())
            .unwrap_or_default();
        anyhow::bail!(
            "no dependency evidence for `{}`: the analyzer gave no usable answer to any of its \
             {asked} definition queries, so a slice would be the seed alone; first: {first}",
            report.seed
        );
    }

    // What the agent would have read instead: every file the slice touched.
    let touched: HashSet<&str> = report.items.iter().map(|i| i.file.as_str()).collect();
    report.source_bytes = touched
        .iter()
        .filter_map(|rel| facts.get(&root.join(rel)).map(|f| f.text.len()))
        .sum();
    Ok(report)
}
