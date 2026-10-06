/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::graph::{TypedGraph, WorkspaceIndex, centrality_score};
use super::tokenizer::tokenize;
use super::types::{DENSE_WEIGHT, Declaration, GRAPH_WEIGHT, HitAttribution, Indexed, POOL, RRF_K};
use prod_code_protocol::SearchHit;
use std::collections::HashMap;

/// Field weights: a query word in the name means more than the same word in a comment.
const W_NAME: f64 = 3.0;
const W_CONTAINER: f64 = 1.5;
const W_SIGNATURE: f64 = 1.2;
const W_DOC: f64 = 1.0;

/// How much a declaration of this kind can answer a question about behaviour. A field named
/// `provider` matches the word, but "where do we decide which provider runs a task" is asking
/// for the code that decides, not for the place the answer is stored.
fn kind_weight(kind: &str) -> f64 {
    match kind {
        "function" | "method" => 1.0,
        "struct" | "class" | "enum" | "interface" | "trait" | "type" | "protocol" => 0.8,
        "impl" | "extension" | "module" => 0.6,
        _ => 0.4,
    }
}

/// The lexical ranking alone.
#[cfg(test)]
pub(crate) fn rank(
    index: &WorkspaceIndex,
    query: &str,
    limit: usize,
    subpath: Option<&str>,
) -> Vec<SearchHit> {
    rank_with(index, query, None, limit, subpath)
}

/// The declarations a question can find: those under `subpath`, and tests only when the
/// question is about tests. A test's name repeats every word of the thing it tests, so on a
/// question about that thing it would outrank the thing itself.
pub(crate) fn candidates<'a>(
    index: &'a WorkspaceIndex,
    terms: &[String],
    subpath: Option<&str>,
) -> Vec<&'a Indexed> {
    let wants_tests = terms
        .iter()
        .any(|t| matches!(t.as_str(), "test" | "spec" | "fixture" | "mock"));
    index
        .declarations()
        .filter(|d| subpath.is_none_or(|p| path_is_in_scope(&d.decl.file, p)))
        .filter(|d| wants_tests || !d.decl.is_test)
        .collect()
}

/// Whether a workspace-relative indexed file is the scope itself or one of its descendants.
/// Search paths are normalized before this is called, so a separator is the only valid
/// component boundary and string prefixes such as `src/foo.rs` never match `src/foo.rsx`.
pub(crate) fn path_is_in_scope(file: &str, scope: &str) -> bool {
    file == scope
        || file
            .strip_prefix(scope)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

/// The lexical ranking, and the dense one when a query vector is given, fused by reciprocal
/// rank: a declaration scores 1 / (60 + its rank) in each list it is in.
pub(crate) fn rank_with(
    index: &WorkspaceIndex,
    query: &str,
    query_vector: Option<&[f32]>,
    limit: usize,
    subpath: Option<&str>,
) -> Vec<SearchHit> {
    rank_weighted(index, query, query_vector, limit, subpath, DENSE_WEIGHT)
}

/// [`rank_with`] with the dense list's weight in the fusion given.
pub(crate) fn rank_weighted(
    index: &WorkspaceIndex,
    query: &str,
    query_vector: Option<&[f32]>,
    limit: usize,
    subpath: Option<&str>,
    dense_weight: f64,
) -> Vec<SearchHit> {
    let terms = tokenize(query);
    let docs = candidates(index, &terms, subpath);
    let fallback_graph;
    let graph =
        if index.graph.in_degree.is_empty() && index.graph.mentions.is_empty() && !index.is_empty()
        {
            fallback_graph = index.build_graph();
            &fallback_graph
        } else {
            &index.graph
        };

    let mut fused: Vec<HitAttribution<'_>> = Vec::new();

    // 1. Lexical BM25
    let lex_results = lexical(&docs, &terms);
    for (rank, (decl, _lex_score, matched_terms)) in lex_results.iter().take(POOL).enumerate() {
        let rrf = 1.0 / (RRF_K + rank as f64 + 1.0);
        let reason = if matched_terms.is_empty() {
            format!("lexical: rank {}", rank + 1)
        } else {
            format!(
                "lexical: rank {} (matched {})",
                rank + 1,
                matched_terms.join(", ")
            )
        };
        match fused.iter_mut().find(|h| std::ptr::eq(h.decl, *decl)) {
            Some(h) => {
                h.total_score += rrf;
                h.reasons.push(reason);
            }
            None => fused.push(HitAttribution {
                decl,
                total_score: rrf,
                reasons: vec![reason],
            }),
        }
    }

    // 2. Typed AST Graph
    let graph_results = graph_rank(&docs, &terms, graph);
    for (rank, (decl, _graph_score, graph_reason)) in graph_results.iter().take(POOL).enumerate() {
        let rrf = GRAPH_WEIGHT / (RRF_K + rank as f64 + 1.0);
        let reason = format!("graph: rank {} ({graph_reason})", rank + 1);
        match fused.iter_mut().find(|h| std::ptr::eq(h.decl, *decl)) {
            Some(h) => {
                h.total_score += rrf;
                h.reasons.push(reason);
            }
            None => fused.push(HitAttribution {
                decl,
                total_score: rrf,
                reasons: vec![reason],
            }),
        }
    }

    // 3. Dense semantic vectors
    if let Some(q) = query_vector {
        let dense_results = dense(&docs, q);
        for (rank, (decl, cosine)) in dense_results.iter().take(POOL).enumerate() {
            let rrf = dense_weight / (RRF_K + rank as f64 + 1.0);
            let reason = format!("dense: rank {} (cosine {:.3})", rank + 1, cosine);
            match fused.iter_mut().find(|h| std::ptr::eq(h.decl, *decl)) {
                Some(h) => {
                    h.total_score += rrf;
                    h.reasons.push(reason);
                }
                None => fused.push(HitAttribution {
                    decl,
                    total_score: rrf,
                    reasons: vec![reason],
                }),
            }
        }
    }

    fused.sort_by(|a, b| {
        b.total_score
            .partial_cmp(&a.total_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.decl.file.cmp(&b.decl.file))
            .then_with(|| a.decl.line.cmp(&b.decl.line))
    });

    fused
        .into_iter()
        .take(limit.max(1))
        .map(|h| SearchHit {
            file: h.decl.file.clone(),
            line: h.decl.line,
            kind: h.decl.kind.clone(),
            name: h.decl.name.clone(),
            container: h.decl.container.clone(),
            signature: h.decl.signature.clone(),
            doc: first_sentence(&h.decl.doc),
            score: Some(format!("{:.4}", h.total_score)),
            rank_reasons: Some(h.reasons),
        })
        .collect()
}

/// Ranks candidate declarations by typed AST graph centrality and relationship reinforcement.
pub(crate) fn graph_rank<'a>(
    docs: &[&'a Indexed],
    terms: &[String],
    graph: &TypedGraph,
) -> Vec<(&'a Declaration, f64, String)> {
    if terms.is_empty() || docs.is_empty() {
        return Vec::new();
    }
    let terms_lower: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let mut scored: Vec<(&'a Declaration, f64, String)> = Vec::new();
    for doc in docs {
        let d = &doc.decl;
        let in_deg = graph.in_degree.get(&d.name).copied().unwrap_or(0);
        let centrality = centrality_score(&d.kind, in_deg);

        let (name_tokens, cont_tokens, sig_tokens, doc_tokens) = &doc.fields;
        let mut direct_matches = 0.0;

        for term in &terms_lower {
            if name_tokens.contains(term) {
                direct_matches += 3.0;
            } else if cont_tokens.contains(term) {
                direct_matches += 1.5;
            } else if sig_tokens.contains(term) {
                direct_matches += 1.0;
            } else if doc_tokens.contains(term) {
                direct_matches += 0.5;
            }
        }

        let mut neighbor_matches = 0.0;
        if let Some(targets) = graph.mentions.get(&d.name) {
            for target in targets {
                for term in &terms_lower {
                    if target.eq_ignore_ascii_case(term) {
                        neighbor_matches += 1.0;
                    }
                }
            }
        }
        if let Some(referrers) = graph.referenced_by.get(&d.name) {
            for referrer in referrers {
                for term in &terms_lower {
                    if referrer.eq_ignore_ascii_case(term) {
                        neighbor_matches += 0.8;
                    }
                }
            }
        }

        let total_match = direct_matches + 0.5 * neighbor_matches;
        if total_match > 0.0 {
            let score = centrality * total_match;
            let reason = format!(
                "{} '{}' in-degree {} (centrality {:.2})",
                d.kind, d.name, in_deg, centrality
            );
            scored.push((d, score, reason));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.file.cmp(&b.0.file))
            .then_with(|| a.0.line.cmp(&b.0.line))
    });
    scored.truncate(POOL);
    scored
}

/// The declarations with a vector, by cosine with the question's.
pub(crate) fn dense<'a>(docs: &[&'a Indexed], query: &[f32]) -> Vec<(&'a Declaration, f32)> {
    let mut scored: Vec<(f32, &'a Declaration)> = docs
        .iter()
        .filter_map(|d| {
            d.vector
                .as_deref()
                .map(|v| (crate::embed::dot(query, v), &d.decl))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(POOL).map(|(s, d)| (d, s)).collect()
}

/// BM25 over the four fields of every declaration, best first.
pub(crate) fn lexical<'a>(
    docs: &[&'a Indexed],
    terms: &[String],
) -> Vec<(&'a Declaration, f64, Vec<String>)> {
    if terms.is_empty() || docs.is_empty() {
        return Vec::new();
    }
    // Document frequency per term, over declarations rather than files.
    let mut df: HashMap<&str, usize> = HashMap::new();
    for (name, container, signature, doc) in docs.iter().map(|d| &d.fields) {
        let mut present: Vec<&str> = Vec::new();
        for t in name.iter().chain(container).chain(signature).chain(doc) {
            if !present.contains(&t.as_str()) {
                present.push(t.as_str());
            }
        }
        for t in present {
            for term in terms {
                if term == t {
                    *df.entry(term.as_str()).or_insert(0) += 1;
                }
            }
        }
    }
    let n = docs.len() as f64;
    let avg_len: f64 = docs.iter().map(|d| d.len).sum::<f64>() / n;
    const K1: f64 = 1.2;
    const B: f64 = 0.45;
    let mut scored: Vec<(f64, &'a Declaration, Vec<String>)> = Vec::new();
    for doc in docs.iter() {
        let (name, container, signature, docs_t) = &doc.fields;
        let len = doc.len;
        let mut score = 0.0;
        let mut matched = 0usize;
        let mut matched_terms = Vec::new();
        for term in terms {
            let tf = W_NAME * count(name, term)
                + W_CONTAINER * count(container, term)
                + W_SIGNATURE * count(signature, term)
                + W_DOC * count(docs_t, term);
            if tf == 0.0 {
                continue;
            }
            matched += 1;
            matched_terms.push(term.clone());
            let df_t = *df.get(term.as_str()).unwrap_or(&1) as f64;
            let idf = ((n - df_t + 0.5) / (df_t + 0.5) + 1.0).ln();
            score += idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * len / avg_len.max(1.0)));
        }
        if matched == 0 {
            continue;
        }
        // A declaration matching more of the question beats one matching one word often.
        score *= 1.0 + 0.35 * (matched - 1) as f64;
        score *= kind_weight(&doc.decl.kind);
        scored.push((score, &doc.decl, matched_terms));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.file.cmp(&b.1.file))
            .then_with(|| a.1.line.cmp(&b.1.line))
    });
    scored.truncate(POOL);
    scored.into_iter().map(|(s, d, m)| (d, s, m)).collect()
}

fn count(tokens: &[String], term: &str) -> f64 {
    tokens.iter().filter(|t| t.as_str() == term).count() as f64
}

/// The first sentence of a doc block, for a one-line result.
pub fn first_sentence(doc: &str) -> String {
    let trimmed = doc.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.find(". ") {
        Some(i) if i < 200 => trimmed[..=i].trim().to_string(),
        _ => trimmed
            .chars()
            .take(200)
            .collect::<String>()
            .trim()
            .to_string(),
    }
}
