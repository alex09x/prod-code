/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;

use anyhow::Result;

use crate::extract_delegate::common::{is_ident, is_ident_str};

pub struct TsMethod {
    pub vis: String,
    pub is_async: bool,
    pub params: String,
    pub ret_type: Option<String>,
    pub full_text: String,
    pub body: String,
    pub start_line: usize,
    pub end_line: usize,
}

pub fn parse_ts_methods(
    body: &str,
    methods: &[String],
    owner: &str,
    fields: &[String],
    helper: &str,
) -> Result<BTreeMap<String, TsMethod>> {
    let mut moved_methods: BTreeMap<String, TsMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < body.len() {
        let slice = &body[offset..];
        let Some(paren_rel) = slice.find('(') else {
            break;
        };
        let paren_pos = offset + paren_rel;
        let before_paren = body[offset..paren_pos].trim();
        let last_word = before_paren.split_whitespace().last().unwrap_or("");
        if is_ident_str(last_word)
            && last_word != "constructor"
            && last_word != "if"
            && last_word != "while"
            && last_word != "for"
            && last_word != "switch"
            && let Some(close_paren_rel) = body[paren_pos..].find(')')
        {
            let close_paren = paren_pos + close_paren_rel;
            let params = body[paren_pos + 1..close_paren].trim().to_string();
            let after_close = &body[close_paren + 1..];
            if let Some(open_b_rel) = after_close.find('{') {
                let open_b = close_paren + 1 + open_b_rel;
                let between = body[close_paren + 1..open_b].trim();
                let ret_type = between.strip_prefix(':').map(|s| s.trim().to_string());
                if let Some(close_b_rel) =
                    crate::parameter_object::matching_bracket(&body[open_b..], 0)
                {
                    let close_b = open_b + close_b_rel;
                    let line_start = body
                        [..offset + slice[..paren_rel].rfind('\n').map_or(0, |x| x + 1)]
                        .rfind('\n')
                        .map_or(0, |x| x + 1);
                    let m_start = body[line_start..paren_pos]
                        .find(last_word)
                        .map(|x| line_start + x)
                        .unwrap_or(paren_pos - last_word.len());
                    let header_part = body[line_start..m_start].trim();
                    let is_async = header_part.contains("async");
                    let vis = if header_part.contains("private") {
                        "private".to_string()
                    } else if header_part.contains("protected") {
                        "protected".to_string()
                    } else if header_part.contains("public") {
                        "public".to_string()
                    } else {
                        String::new()
                    };
                    let method_text = body[line_start..=close_b].trim().to_string();
                    let m_body = body[open_b + 1..close_b].to_string();
                    let start_line = body[..line_start].matches('\n').count();
                    let end_line = body[..close_b].matches('\n').count();
                    let method_name = last_word.to_string();
                    if methods.contains(&method_name) {
                        moved_methods.insert(
                            method_name,
                            TsMethod {
                                vis,
                                is_async,
                                params,
                                ret_type,
                                full_text: method_text,
                                body: m_body,
                                start_line,
                                end_line,
                            },
                        );
                    }
                    offset = close_b + 1;
                    continue;
                }
            }
        }
        offset = paren_pos + 1;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for (idx, _) in minfo.body.match_indices("this.") {
            let after = &minfo.body[idx + 5..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty()
                && ident != "constructor"
                && !fields.contains(&ident)
                && !methods.contains(&ident)
            {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    Ok(moved_methods)
}
