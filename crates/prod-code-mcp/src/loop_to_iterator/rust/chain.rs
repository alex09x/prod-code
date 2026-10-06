/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::loop_to_iterator::helpers::is_ident;
use crate::loop_to_iterator::types::{AccumulatorLoop, Shape};

/// What `for P in E` iterates, written as an iterator: `E.into_iter()`, `v.iter()` for `&v`,
/// and a range as it is.
pub fn iterator_of(source: &str, source_type: Option<&str>) -> String {
    let simple = |s: &str| !s.is_empty() && s.chars().all(|c| is_ident(c) || c == '.');
    if let Some(p) = source.strip_prefix("&mut ").filter(|p| simple(p)) {
        return format!("{p}.iter_mut()");
    }
    if let Some(p) = source.strip_prefix('&').filter(|p| simple(p)) {
        return format!("{p}.iter()");
    }
    // A name that holds a reference (`prices: &[u64]`): `into_iter` on it is `iter`, and clippy
    // says so (`into_iter_on_ref`).
    if simple(source) {
        match source_type {
            Some(t) if t.starts_with("&mut ") => return format!("{source}.iter_mut()"),
            Some(t) if t.starts_with('&') => return format!("{source}.iter()"),
            _ => {}
        }
    }
    if source.contains("..") {
        return format!("({source})");
    }
    if simple(source) || source.ends_with(')') && !source.contains(' ') {
        return format!("{source}.into_iter()");
    }
    format!("({source}).into_iter()")
}

/// The statement that replaces the `let` and the loop.
pub fn chain(l: &AccumulatorLoop, ty: &str, source_type: Option<&str>, mutable: bool) -> String {
    let p = &l.pattern;
    let simple = !p.is_empty() && p.chars().all(is_ident);
    let steps = match &l.shape {
        Shape::Sum { cond: None, value } if value == p => ".sum()".to_string(),
        Shape::Sum { cond: None, value } => format!(".map(|{p}| {value}).sum()"),
        Shape::Sum {
            cond: Some(c),
            value,
        } => {
            format!(".filter_map(|{p}| if {c} {{ Some({value}) }} else {{ None }}).sum()")
        }
        Shape::Count { cond } if simple => format!(".filter(|&{p}| {cond}).count()"),
        Shape::Count { cond } => {
            format!(".filter_map(|{p}| if {cond} {{ Some(()) }} else {{ None }}).count()")
        }
        Shape::Collect { cond: None, value } if value == p => ".collect()".to_string(),
        Shape::Collect { cond: None, value } => format!(".map(|{p}| {value}).collect()"),
        Shape::Collect {
            cond: Some(c),
            value,
        } => {
            format!(".filter_map(|{p}| if {c} {{ Some({value}) }} else {{ None }}).collect()")
        }
        Shape::Find { cond, value } if value == p && simple => {
            format!(".find(|&{p}| {cond})")
        }
        Shape::Find { cond, value } => {
            format!(".find_map(|{p}| if {cond} {{ Some({value}) }} else {{ None }})")
        }
        Shape::Any { cond } if simple => {
            format!(".any(|&{p}| {cond})")
        }
        Shape::Any { cond } => {
            format!(".any(|{p}| {cond})")
        }
        Shape::All { cond } if simple => {
            format!(".all(|&{p}| {cond})")
        }
        Shape::All { cond } => {
            format!(".all(|{p}| {cond})")
        }
    };
    format!(
        "{}let {}{}: {ty} = {}{steps};",
        l.indent,
        if mutable { "mut " } else { "" },
        l.acc,
        iterator_of(&l.source, source_type)
    )
}
