/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tokenizer::tokenize;
use prod_code_protocol::{DenseStatus, SearchHit};

/// Declarations kept per file; a file with more is truncated (generated code, big tables).
pub const MAX_DECLS_PER_FILE: usize = 400;
/// Files larger than this are not indexed.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Doc-comment lines collected above a declaration.
pub const MAX_DOC_LINES: usize = 12;
/// Hits returned when the caller does not say.
pub const DEFAULT_LIMIT: usize = 10;
/// How deep each half's ranking goes before the two are fused.
pub const POOL: usize = 50;
/// Reciprocal-rank fusion's constant: a hit at rank r scores 1 / (RRF_K + r) in each list.
pub const RRF_K: f64 = 60.0;
/// How much the dense list counts against the lexical one in the fusion. Chosen by
/// `eval_ranking_on_this_repository`.
pub const DENSE_WEIGHT: f64 = 1.0;
/// Declarations embedded per model call.
pub const EMBED_BATCH: usize = 64;
/// Characters of a declaration handed to the model.
pub const MAX_PASSAGE_CHARS: usize = 1000;
/// Weight given to the graph score in fusion.
pub const GRAPH_WEIGHT: f64 = 0.9;

/// A declaration's four searchable fields, tokenized: name, container, signature, doc.
pub type Fields = (Vec<String>, Vec<String>, Vec<String>, Vec<String>);

/// One indexed declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub file: String,
    pub line: u32,
    pub kind: String,
    pub name: String,
    pub container: Option<String>,
    pub signature: String,
    pub doc: String,
    /// The declaration belongs to a test: its name, its container, its file, or simply its
    /// position after the file's test module began.
    pub is_test: bool,
}

/// A declaration with its fields already tokenized: a query then costs one pass over the
/// index instead of retokenizing every declaration it looks at.
pub struct Indexed {
    pub decl: Declaration,
    pub fields: Fields,
    pub len: f64,
    /// The dense vector, once the background pass has computed it.
    pub vector: Option<Vec<f32>>,
}

impl Indexed {
    pub fn new(decl: Declaration) -> Self {
        let fields = (
            tokenize(&decl.name),
            tokenize(decl.container.as_deref().unwrap_or("")),
            tokenize(&decl.signature),
            tokenize(&decl.doc),
        );
        let len = (fields.0.len() + fields.1.len() + fields.2.len() + fields.3.len()) as f64;
        Self {
            decl,
            fields,
            len,
            vector: None,
        }
    }

    /// What the model reads for this declaration: its kind, its name as words, where it lives,
    /// its signature and its doc comment.
    pub fn passage(&self) -> String {
        let d = &self.decl;
        let mut text = format!("{} {}", d.kind, self.fields.0.join(" "));
        if let Some(container) = &d.container {
            text.push_str(&format!(" in {container}"));
        }
        text.push_str(&format!(". {}. {}", d.signature, d.doc));
        text.chars().take(MAX_PASSAGE_CHARS).collect()
    }
}

/// What a file contributed, with the stamp that tells us whether to redo it.
pub struct FileEntry {
    pub stamp: (u64, u64),
    pub generation: u64,
    pub decls: Vec<Indexed>,
}

/// What one search found, and over how much.
pub struct Found {
    pub hits: Vec<SearchHit>,
    pub files: usize,
    pub declarations: usize,
    /// `None` when there is no model: the ranking was lexical only.
    pub dense: Option<DenseStatus>,
    /// Whether typed graph fusion was applied during search ranking.
    pub graph_fused: bool,
}

pub struct HitAttribution<'a> {
    pub decl: &'a Declaration,
    pub total_score: f64,
    pub reasons: Vec<String>,
}
