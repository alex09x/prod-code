/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use serde_json::{Value, json};

pub(super) fn symbols(source: &str) -> Value {
    let items: Vec<Value> = source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            function_name(line).map(|name| {
                let col = line.find(name).unwrap_or(0) as u64;
                json!({
                    "name": name,
                    "kind": 12,
                    "range": {
                        "start": {"line": index, "character": col},
                        "end": {"line": index, "character": line.len()}
                    },
                    "selectionRange": {
                        "start": {"line": index, "character": col},
                        "end": {"line": index, "character": col + name.len() as u64}
                    }
                })
            })
        })
        .collect();
    Value::Array(items)
}

fn function_name(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let candidate = if let Some(rest) = line.strip_prefix("function ") {
        rest.split(['(', ' ', '\t', '{']).next()?
    } else {
        let (name, rest) = line.split_once("()")?;
        if !rest.trim_start().starts_with('{') {
            return None;
        }
        name.trim()
    };
    let mut chars = candidate.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        .then_some(())?;
    chars
        .all(|c| c == '_' || c.is_ascii_alphanumeric())
        .then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::symbols;

    #[test]
    fn outlines_bash_function_forms_and_skips_comments_and_calls() {
        let result = symbols(
            "# fake() {\nplain() {\n  echo ok\n}\nfunction with_args {\n  echo ok\n}\nplain\n",
        );
        let names: Vec<&str> = result
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["name"].as_str())
            .collect();
        assert_eq!(names, ["plain", "with_args"]);
    }
}
