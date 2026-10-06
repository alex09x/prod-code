/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyTokenKind {
    Word,
    Number,
    Str,
    Char,
    Punct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolyToken {
    pub kind: PolyTokenKind,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedParam {
    pub name: String,
    pub ty: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputKind {
    Expression(String),
    EndsWithReturn,
    SingleVar { name: String, is_new: bool },
    MultipleVars(Vec<String>),
    Void,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolyOccurrence {
    pub start: usize,
    pub end: usize,
    pub differs: Vec<(usize, String)>,
}
