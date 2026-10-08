/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, anyhow};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use url::Url;

use super::call_sites::check_signature_warnings;
use super::diff::{Change, diff_hunks};
use super::incoming::{Incoming, incoming_calls, note, test_name, unreadable};
use super::pool::SessionPool;
use super::signatures::check_adjusted_signature;
use super::symbols::{collect_functions, is_source_file};
use super::test_cmd::{file_language, test_command_for_tests};
use super::types::{COLD_RETRIES, COLD_WAIT, Gap, ImpactReport, IndexBuild, Reach, Symbol};

async fn swift_index_build(remote: SocketAddr, root: &Path) -> Option<IndexBuild> {
    let command = vec![
        "swift".to_string(),
        "build".to_string(),
        "--build-tests".to_string(),
    ];
    let outcome = crate::exec::run_remote(
        remote,
        root,
        None,
        command.clone(),
        Vec::new(),
        900,
        false,
        |_, _| {},
    )
    .await;
    Some(IndexBuild {
        command: command.join(" "),
        ok: outcome
            .as_ref()
            .is_ok_and(|o| o.exit.exit_code == Some(0) && !o.exit.timed_out),
        duration_ms: outcome.as_ref().map_or(0, |o| o.exit.duration_ms),
    })
}

/// Runs the analysis against the checkout at `root` placed on `remote`.
pub async fn analyze(
    remote: SocketAddr,
    root: &Path,
    base: Option<&str>,
    depth: usize,
) -> Result<ImpactReport> {
    let language = crate::sync::expected_engine(root)
        .ok_or_else(|| anyhow!("no project manifest at {}", root.display()))?
        .to_string();
    let tools = crate::verify::detect_tools(root);
    let changes = diff_hunks(root, base)?;
    let index = if language == "swift" && !changes.is_empty() {
        swift_index_build(remote, root).await
    } else {
        None
    };
    let mut session_pool = SessionPool::new(remote, root);
    let changed_files: Vec<String> = changes.keys().cloned().collect();
    let mut changed: Vec<Symbol> = Vec::new();
    let mut unattributed: Vec<String> = Vec::new();
    let mut incomplete: Vec<Gap> = Vec::new();
    let mut adjusted_signatures: Vec<(Symbol, String, String, u32, u32)> = Vec::new();

    for (file, change) in &changes {
        if !is_source_file(file) {
            unattributed.push(file.clone());
            continue;
        }
        let file_hunks = match change {
            Change::Hunks(hunks) => hunks,
            Change::Unknown(error) => {
                let (file, error) = (file.clone(), error.clone());
                note(&mut incomplete, Gap::Diff { file, error });
                continue;
            }
        };
        let abs: PathBuf = root.join(file);
        // A deleted file's functions are gone, and whoever called them changed with them.
        if !abs.exists() {
            note(&mut incomplete, Gap::Deleted { file: file.clone() });
            continue;
        }
        if file_hunks.is_empty() {
            continue; // git counts no changed line: only its mode changed
        }
        let text = match std::fs::read_to_string(&abs) {
            Ok(text) => text,
            Err(e) => {
                let error = format!("it cannot be read: {e}");
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
        };
        let file_lang = file_language(root, file).unwrap_or(&language);
        let session = match session_pool.session_for_file(&abs).await {
            Ok(s) => s,
            Err(e) => {
                let error = format!("{e:#}");
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("bad path {file}"))?
            .to_string();
        let symbols = match session
            .query(
                &abs,
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
        {
            Ok(serde_json::Value::Array(symbols)) => symbols,
            res => {
                let error = match res {
                    Ok(serde_json::Value::Null) => {
                        "textDocument/documentSymbol answered null, so its functions are unknown"
                            .to_string()
                    }
                    Ok(other) => unreadable("textDocument/documentSymbol", &other),
                    Err(e) => format!("{e:#}"),
                };
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
        };
        let mut functions = Vec::new();
        if let Err(error) = collect_functions(&symbols, Some(&text), &mut functions) {
            let file = file.clone();
            note(&mut incomplete, Gap::Symbols { file, error });
            continue;
        }
        for (name, start, end, sl, sc) in &functions {
            if file_hunks.iter().any(|h| h.touches(*start, *end)) {
                let sym = Symbol {
                    name: name.clone(),
                    file: file.clone(),
                    line: *sl,
                    col: *sc,
                };
                if !changed.contains(&sym) {
                    changed.push(sym.clone());
                }

                if let Some(adj) = check_adjusted_signature(
                    root, file, base, file_hunks, &text, name, file_lang, *sl, &sym,
                ) {
                    adjusted_signatures.push(adj);
                }
            }
        }
        // Each hunk on its own: a changed function does not vouch for a changed import or
        // constant elsewhere in the same file.
        let spans: Vec<(u32, u32)> = functions.iter().map(|f| (f.1, f.2)).collect();
        let lines: Vec<&str> = text.lines().collect();
        if file_hunks.iter().any(|h| !h.inside(&spans, &lines)) {
            unattributed.push(file.clone());
        }
    }

    // A changed test is affected by its own change, whatever calls it.
    let mut tests: BTreeSet<Symbol> = BTreeSet::new();
    let mut reaches: Vec<Reach> = Vec::new();
    let mut origins: Vec<(Symbol, bool)> = Vec::with_capacity(changed.len());
    for sym in &changed {
        let sym_file_lang = file_language(root, &sym.file).unwrap_or(&language);
        let is_test = match test_name(root, sym_file_lang, &sym.name, &sym.file, sym.line, false) {
            Ok(Some(name)) => {
                let test = Symbol {
                    name,
                    ..sym.clone()
                };
                tests.insert(test.clone());
                reaches.push(Reach {
                    test,
                    changed: sym.clone(),
                    hops: 0,
                });
                true
            }
            Ok(None) => false,
            Err(error) => {
                note(
                    &mut incomplete,
                    Gap::Callers {
                        symbol: sym.clone(),
                        error,
                    },
                );
                false
            }
        };
        origins.push((sym.clone(), is_test));
    }

    // Walk incoming calls breadth-first from each changed function on its own, so every test
    // reached knows which changed functions reach it and in how many hops. The answers are
    // cached: a function two walks pass through is asked once.
    let key = |s: &Symbol| (s.file.clone(), s.line, s.col);
    let changed_keys: HashSet<(String, u32, u32)> = changed.iter().map(key).collect();
    let mut cache: HashMap<(String, u32, u32), Incoming> = HashMap::new();
    // A language server that has just started answers the call hierarchy with nothing until it
    // has read the project, and "no callers" would then read as "no test is affected" (#202).
    // For the managed servers, an empty answer for the first changed function is asked again a
    // few times before it is believed; rust-analyzer answers from a database already loaded.
    if let Some(first) = changed.first() {
        let first_abs = root.join(&first.file);
        let first_lang = file_language(root, &first.file).unwrap_or(&language);
        if first_lang != "rust"
            && let Ok(first_session) = session_pool.session_for_file(&first_abs).await
        {
            let mut answer = incoming_calls(first_session, root, first_lang, first, None).await;
            for _ in 0..COLD_RETRIES {
                if matches!(&answer, Incoming::Callers(found) if !found.is_empty()) {
                    break;
                }
                tokio::time::sleep(COLD_WAIT).await;
                if let Ok(first_session) = session_pool.session_for_file(&first_abs).await {
                    answer = incoming_calls(first_session, root, first_lang, first, None).await;
                }
            }
            cache.insert(key(first), answer);
        }
    }
    const IMPACT_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);
    let bfs_start = tokio::time::Instant::now();
    let deadline = bfs_start + IMPACT_BUDGET;
    let mut callers: BTreeSet<Symbol> = BTreeSet::new();
    let mut timed_out = false;
    let check_budget = |incomplete: &mut Vec<Gap>| -> bool {
        let elapsed = bfs_start.elapsed();
        if elapsed < IMPACT_BUDGET {
            return false;
        }
        let (elapsed_ms, budget_ms) =
            (elapsed.as_millis() as u64, IMPACT_BUDGET.as_millis() as u64);
        note(
            incomplete,
            Gap::Timeout {
                elapsed_ms,
                budget_ms,
            },
        );
        true
    };
    for (origin, origin_is_test) in &origins {
        if *origin_is_test {
            continue;
        }
        if check_budget(&mut incomplete) {
            break;
        }
        let mut seen: HashSet<(String, u32, u32)> = HashSet::from([key(origin)]);
        let mut queue: VecDeque<(Symbol, bool, usize)> =
            VecDeque::from([(origin.clone(), *origin_is_test, 0)]);
        while let Some((sym, is_test, level)) = queue.pop_front() {
            if check_budget(&mut incomplete) {
                timed_out = true;
                break;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(key(&sym)) {
                let sym_abs = root.join(&sym.file);
                let incoming = match session_pool.session_for_file(&sym_abs).await {
                    Ok(sym_session) => {
                        let sym_lang = file_language(root, &sym.file).unwrap_or(&language);
                        incoming_calls(sym_session, root, sym_lang, &sym, Some(deadline)).await
                    }
                    Err(e) => Incoming::Failed(format!("{e:#}")),
                };
                slot.insert(incoming);
            }
            let found = match &cache[&key(&sym)] {
                Incoming::Callers(found) => found.clone(),
                // A test is selected whatever calls it, and module-level test code (a test
                // file's top-level `it(...)`) has no item of its own.
                Incoming::NoItem if is_test => Vec::new(),
                Incoming::NoItem => {
                    let error = "the analyzer has no call-hierarchy item at its name".to_string();
                    note(&mut incomplete, Gap::Callers { symbol: sym, error });
                    continue;
                }
                Incoming::Failed(error) => {
                    let error = error.clone();
                    note(&mut incomplete, Gap::Callers { symbol: sym, error });
                    continue;
                }
            };
            if level >= depth {
                // Asked one level further, the answer says whether the limit cut the walk
                // short: a caller not yet seen is a test path left unexplored.
                if found.iter().any(|(caller, _)| !seen.contains(&key(caller))) {
                    note(&mut incomplete, Gap::Depth { symbol: sym, depth });
                }
                continue;
            }

            const MAX_FAN_IN: usize = 30;
            if found.len() > MAX_FAN_IN {
                note(
                    &mut incomplete,
                    Gap::FanIn {
                        symbol: sym.clone(),
                        callers: found.len(),
                        limit: MAX_FAN_IN,
                    },
                );
                continue;
            }

            for (caller, caller_is_test) in found {
                if !seen.insert(key(&caller)) {
                    continue;
                }
                if caller_is_test {
                    tests.insert(caller.clone());
                    reaches.push(Reach {
                        test: caller.clone(),
                        changed: origin.clone(),
                        hops: level + 1,
                    });
                } else {
                    if !changed_keys.contains(&key(&caller)) {
                        callers.insert(caller.clone());
                    }
                    queue.push_back((caller, caller_is_test, level + 1));
                }
            }
        }
        if timed_out {
            break;
        }
    }

    let signature_warnings =
        check_signature_warnings(&mut session_pool, root, &changes, adjusted_signatures).await;

    session_pool.close_all().await;
    let tests: Vec<Symbol> = tests.into_iter().collect();
    let test_command = test_command_for_tests(root, &language, &tools, &tests);
    Ok(ImpactReport {
        language,
        base: base.unwrap_or("HEAD").to_string(),
        changed_files,
        changed,
        callers: callers.into_iter().collect(),
        tests,
        test_command,
        unattributed_files: unattributed,
        index,
        reaches,
        incomplete,
        signature_warnings,
    })
}
