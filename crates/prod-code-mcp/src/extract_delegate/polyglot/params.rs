/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_delegate::common::{is_ident_str, split_top};

pub fn extract_param_names_ts(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let before_colon = p.split(':').next().unwrap_or(p).trim();
        let name = before_colon
            .split_whitespace()
            .last()
            .unwrap_or("")
            .trim_start_matches("...");
        if is_ident_str(name) {
            names.push(name.to_string());
        }
    }
    names
}

pub fn extract_param_names_py(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut keyword_only = false;
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() || p == "/" {
            continue;
        }
        if p == "*" {
            keyword_only = true;
            continue;
        }
        if p == "self" || p.starts_with("self:") {
            continue;
        }
        let (marker, parameter) = if let Some(rest) = p.strip_prefix("**") {
            ("**", rest)
        } else if let Some(rest) = p.strip_prefix('*') {
            keyword_only = true;
            ("*", rest)
        } else {
            ("", p)
        };
        let before_equal = parameter.split('=').next().unwrap_or(parameter).trim();
        let before_colon = before_equal
            .split(':')
            .next()
            .unwrap_or(before_equal)
            .trim();
        let name = before_colon;
        if is_ident_str(name) {
            if marker == "**" {
                names.push(format!("**{name}"));
            } else if marker == "*" {
                names.push(format!("*{name}"));
            } else if keyword_only {
                names.push(format!("{name}={name}"));
            } else {
                names.push(name.to_string());
            }
        }
    }
    names
}

pub fn extract_param_names_cpp(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let before_equal = p.split('=').next().unwrap_or(p).trim();
        let name = before_equal
            .split_whitespace()
            .last()
            .unwrap_or("")
            .trim_matches(['&', '*']);
        if is_ident_str(name) {
            names.push(name.to_string());
        }
    }
    names
}

pub fn extract_param_names_swift(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let before_colon = p.split(':').next().unwrap_or(p).trim();
        let parts: Vec<&str> = before_colon.split_whitespace().collect();
        if parts.len() >= 2 {
            let label = parts[0];
            let name = parts[1];
            if label == "_" {
                names.push(name.to_string());
            } else {
                names.push(format!("{label}: {name}"));
            }
        } else if let Some(name) = parts.first()
            && is_ident_str(name)
        {
            names.push(format!("{name}: {name}"));
        }
    }
    names
}

pub fn extract_param_names_go(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let name = p.split_whitespace().next().unwrap_or("");
        if is_ident_str(name) {
            names.push(name.to_string());
        }
    }
    names
}
