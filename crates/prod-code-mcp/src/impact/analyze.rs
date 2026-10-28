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
use std::path::Path;

use super::budget::{IMPACT_BUDGET, expired as budget_expired, within_deadline};
use super::call_sites::check_signature_warnings;
use super::changed::scan_changed_files;
use super::diff::diff_hunks;
use super::incoming::{Incoming, incoming_calls, note, test_name};
use super::index::swift_index_build;
use super::pool::SessionPool;
use super::test_cmd::file_language;
use super::types::{COLD_RETRIES, COLD_WAIT, Gap, ImpactReport, Reach, Symbol};

/// Runs the analysis against the checkout at `root` placed on `remote`.
pub async fn analyze(
    remote: SocketAddr,
    root: &Path,
    base: Option<&str>,
    depth: usize,
) -> Result<ImpactReport> {
    let analysis_started = tokio::time::Instant::now();
    let deadline = analysis_started + IMPACT_BUDGET;
    let language = crate::sync::expected_engine(root)
        .ok_or_else(|| anyhow!("no project manifest at {}", root.display()))?
        .to_string();
    let tools = crate::verify::detect_tools(root);
    let changes = diff_hunks(root, base)?;
    let mut timed_out = false;
    let index = if language == "swift" && !changes.is_empty() {
        match within_deadline(deadline, swift_index_build(remote, root)).await {
            Ok(index) => index,
            Err(_) => {
                timed_out = true;
                None
            }
        }
    } else {
        None
    };
    let mut session_pool = SessionPool::new(remote, root);
    let changed_files: Vec<String> = changes.keys().cloned().collect();
    let scan = scan_changed_files(
        root,
        &language,
        base,
        &changes,
        &mut session_pool,
        analysis_started,
        deadline,
    )
    .await?;
    let changed = scan.changed;
    let unattributed = scan.unattributed;
    let adjusted_signatures = scan.adjusted_signatures;
    let mut incomplete = scan.incomplete;
    if timed_out {
        budget_expired(analysis_started, deadline, &mut incomplete);
    }
    timed_out |= scan.timed_out;

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
    if !timed_out && let Some(first) = changed.first() {
        let first_abs = root.join(&first.file);
        let first_lang = file_language(root, &first.file).unwrap_or(&language);
        let first_session = if first_lang == "rust" {
            None
        } else {
            match within_deadline(deadline, session_pool.session_for_file(&first_abs)).await {
                Ok(Ok(session)) => Some(session),
                Ok(Err(_)) => None,
                Err(_) => {
                    budget_expired(analysis_started, deadline, &mut incomplete);
                    timed_out = true;
                    None
                }
            }
        };
        if let Some(first_session) = first_session {
            let mut answer =
                incoming_calls(first_session, root, first_lang, first, Some(deadline)).await;
            for _ in 0..COLD_RETRIES {
                match &answer {
                    Incoming::Callers(found) if !found.is_empty() => break,
                    // A request that failed, timed out or answered out of shape is not a cold
                    // index: asking again only spends the wait, and the failure is kept as the
                    // reason the analysis is incomplete (#1473).
                    Incoming::Failed(_) => break,
                    _ => {}
                }
                if budget_expired(analysis_started, deadline, &mut incomplete) {
                    timed_out = true;
                    break;
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    budget_expired(analysis_started, deadline, &mut incomplete);
                    timed_out = true;
                    break;
                }
                tokio::time::sleep(COLD_WAIT.min(remaining)).await;
                let first_session = match within_deadline(
                    deadline,
                    session_pool.session_for_file(&first_abs),
                )
                .await
                {
                    Ok(Ok(session)) => session,
                    Ok(Err(_)) => break,
                    Err(_) => {
                        budget_expired(analysis_started, deadline, &mut incomplete);
                        timed_out = true;
                        break;
                    }
                };
                answer =
                    incoming_calls(first_session, root, first_lang, first, Some(deadline)).await;
            }
            cache.insert(key(first), answer);
        }
    }
    let mut callers: BTreeSet<Symbol> = BTreeSet::new();
    for (origin, origin_is_test) in &origins {
        if *origin_is_test {
            continue;
        }
        if budget_expired(analysis_started, deadline, &mut incomplete) {
            break;
        }
        let mut seen: HashSet<(String, u32, u32)> = HashSet::from([key(origin)]);
        let mut queue: VecDeque<(Symbol, bool, usize)> =
            VecDeque::from([(origin.clone(), *origin_is_test, 0)]);
        while let Some((sym, is_test, level)) = queue.pop_front() {
            if budget_expired(analysis_started, deadline, &mut incomplete) {
                timed_out = true;
                break;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(key(&sym)) {
                let sym_abs = root.join(&sym.file);
                let incoming = match within_deadline(
                    deadline,
                    session_pool.session_for_file(&sym_abs),
                )
                .await
                {
                    Ok(Ok(sym_session)) => {
                        let sym_lang = file_language(root, &sym.file).unwrap_or(&language);
                        incoming_calls(sym_session, root, sym_lang, &sym, Some(deadline)).await
                    }
                    Ok(Err(e)) => Incoming::Failed(format!("{e:#}")),
                    Err(_) => {
                        budget_expired(analysis_started, deadline, &mut incomplete);
                        timed_out = true;
                        break;
                    }
                };
                slot.insert(incoming);
            }
            if budget_expired(analysis_started, deadline, &mut incomplete) {
                timed_out = true;
                break;
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

    let signature_warnings = if timed_out {
        Vec::new()
    } else {
        match within_deadline(
            deadline,
            check_signature_warnings(&mut session_pool, root, &changes, adjusted_signatures),
        )
        .await
        {
            Ok(warnings) => warnings,
            Err(_) => {
                budget_expired(analysis_started, deadline, &mut incomplete);
                Vec::new()
            }
        }
    };

    session_pool.close_all().await;
    let tests: Vec<Symbol> = tests.into_iter().collect();
    let test_plan =
        super::test_plan::build_test_plan(root, &language, &tools, &tests, &changed_files);
    let test_command = test_plan.selected.clone();
    Ok(ImpactReport {
        language,
        base: base.unwrap_or("HEAD").to_string(),
        changed_files,
        changed,
        callers: callers.into_iter().collect(),
        tests,
        test_command,
        test_plan: Some(test_plan),
        unattributed_files: unattributed,
        index,
        reaches,
        incomplete,
        signature_warnings,
    })
}
