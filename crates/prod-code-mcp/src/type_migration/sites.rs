/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::conversion::{language_conversion, parse_mismatch, suggest, type_name};
use super::spans::expression_span;
use super::types::{Conversion, Converted, Site};
use crate::parameter_object::Language;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The errors of `reports` as sites, collapsed and with the ones on attributes counted apart.
pub(crate) fn collect_sites(
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    reports: &[crate::diagnostics::DiagnosticsReport],
    was: &str,
    to: &str,
) -> (Vec<Site>, usize) {
    let mut sites = Vec::new();
    for report in reports {
        let source_of = |line: u32| -> String {
            let path = root.join(&report.file);
            let text = rewritten
                .iter()
                .find(|(p, _)| **p == path || display(root, p) == report.file)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
            text.lines()
                .nth(line.saturating_sub(1) as usize)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        for item in report.items.iter().chain(&report.in_derive) {
            if item.severity != "error" {
                continue;
            }
            let message = item.message.lines().next().unwrap_or("").to_string();
            sites.push(Site {
                file: report.file.clone(),
                line: item.line,
                col: item.col,
                suggestion: suggest(&message, was, to),
                message,
                code: item.code.clone(),
                source: source_of(item.line),
                end: item.end,
            });
        }
    }
    sites.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.col.cmp(&b.col))
            .then(a.message.cmp(&b.message))
    });
    sites.dedup_by(|a, b| {
        a.file == b.file && a.line == b.line && a.col == b.col && a.message == b.message
    });
    let before = sites.len();
    sites.retain(|s| !s.source.trim_start().starts_with("#["));
    let in_attributes = before - sites.len();
    (sites, in_attributes)
}

/// Whether a language-idiomatic conversion can go on this site's expression.
pub(crate) fn is_candidate(site: &Site, was: &str, now: &str) -> bool {
    if site.file.ends_with(".rs") {
        if site.code.as_deref() != Some("E0308") {
            return false;
        }
        if !site.end.is_some_and(|(line, _)| line == site.line) {
            return false;
        }
    } else {
        let is_candidate_code = site.code.as_deref() == Some("E0308")
            || site.code.as_deref().is_some_and(|c| {
                c.contains("2322")
                    || c.contains("2345")
                    || c.contains("type")
                    || c.contains("error")
            })
            || site.code.is_none()
            || site.message.contains("expected")
            || site.message.contains("not assignable")
            || site.message.contains("cannot convert")
            || site.message.contains("conversion")
            || site.message.contains("cannot use");
        if !is_candidate_code {
            return false;
        }
        if !site.end.is_none_or(|(line, _)| line == site.line) {
            return false;
        }
    }

    let Some((expected, found)) = parse_mismatch(&site.message) else {
        return false;
    };
    let (expected, found) = (type_name(&expected), type_name(&found));
    let (was, now) = (type_name(was), type_name(now));
    (expected == now && found == was) || (expected == was && found == now)
}

pub(crate) async fn convert_sites(
    remote: SocketAddr,
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    also: &[PathBuf],
    sites: &[Site],
    was: &str,
    now: &str,
) -> Result<Converted> {
    let known: BTreeSet<(String, u32, String)> = sites
        .iter()
        .map(|s| (s.file.clone(), s.line, s.message.clone()))
        .collect();
    let mut candidates: Vec<&Site> = sites.iter().filter(|s| is_candidate(s, was, now)).collect();
    let mut tried: BTreeSet<(String, u32, u32)> = BTreeSet::new();
    for _round in 0..4 {
        if candidates.is_empty() {
            return Ok(Converted::Nothing { tried });
        }
        let mut texts = rewritten.clone();
        let mut spans: BTreeMap<PathBuf, Vec<(usize, usize, &Site)>> = BTreeMap::new();
        for site in &candidates {
            let path = root.join(&site.file);
            let key = texts
                .keys()
                .find(|p| **p == path || display(root, p) == site.file)
                .cloned()
                .unwrap_or(path);
            let text = match texts.get(&key) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&key) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            texts.entry(key.clone()).or_insert(text.clone());
            if let Some((start, end)) = expression_span(&text, site) {
                spans.entry(key).or_default().push((start, end, site));
            }
        }
        let mut conversions = Vec::new();
        for (path, mut list) in spans {
            list.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
            let text = texts.get_mut(&path).expect("read above");
            let file_lang = Language::of(&path).unwrap_or(Language::Rust);
            for (start, end, site) in list {
                let expr = text[start..end].to_string();
                let call = language_conversion(&expr, now, file_lang);
                text.replace_range(start..end, &call);
                conversions.push(Conversion {
                    file: site.file.clone(),
                    line: site.line,
                    was: expr,
                    now: call,
                });
            }
        }
        conversions.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        let edits: Vec<(PathBuf, String)> =
            texts.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
        let others: Vec<PathBuf> = also
            .iter()
            .filter(|p| !texts.contains_key(*p))
            .cloned()
            .collect();
        let reports = crate::diagnostics::validate_texts(remote, root, &edits, &others).await?;
        let failing: BTreeSet<(String, u32)> = reports
            .iter()
            .flat_map(|r| {
                r.items
                    .iter()
                    .filter(|d| d.severity == "error")
                    .map(move |d| (r.file.clone(), d.line))
            })
            .collect();
        let before = candidates.len();
        candidates.retain(|s| {
            let bad = failing.contains(&(s.file.clone(), s.line));
            if bad {
                tried.insert((s.file.clone(), s.line, s.col));
            }
            !bad
        });
        if candidates.len() < before {
            continue;
        }
        let converted_lines: BTreeSet<(String, u32)> = conversions
            .iter()
            .map(|c| (c.file.clone(), c.line))
            .collect();
        let new: Vec<String> = reports
            .iter()
            .flat_map(|r| {
                r.items
                    .iter()
                    .filter(|d| d.severity == "error")
                    .map(move |d| (r.file.clone(), d.line, d.message.clone()))
            })
            .filter(|(f, l, m)| {
                let first = m.lines().next().unwrap_or("").to_string();
                !known.contains(&(f.clone(), *l, first))
                    && !converted_lines.contains(&(f.clone(), *l))
            })
            .map(|(f, l, m)| format!("{f}:{l} {}", m.lines().next().unwrap_or("")))
            .collect();
        if !new.is_empty() {
            return Ok(Converted::Dropped {
                note: format!(
                    "{} conversion(s) type-check where they are but cause an error elsewhere, so \
                     none was kept:\n  {}",
                    conversions.len(),
                    new.join("\n  ")
                ),
                tried,
            });
        }
        return Ok(Converted::Accepted {
            texts,
            conversions,
            reports,
            tried,
        });
    }
    Ok(Converted::Dropped {
        note: "the conversions did not settle in four rounds of checking, so none was kept"
            .to_string(),
        tried,
    })
}

pub(crate) fn mark_tried(sites: &mut [Site], tried: &BTreeSet<(String, u32, u32)>) {
    for site in sites {
        if tried.contains(&(site.file.clone(), site.line, site.col)) {
            site.suggestion = Some(
                "A conversion was tried here and does not type-check: there is no conversion the \
                 analyzer accepts, so this one is a decision (a narrowing, a fallible \
                 conversion, or a place that should be migrated too)"
                    .to_string(),
            );
        }
    }
}
