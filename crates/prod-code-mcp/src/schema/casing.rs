/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::Variant;

/// Words of a name, whatever style it is written in: `order_id`, `orderId`, `OrderID`,
/// `ORDER_ID` and `order-id` all give `["order", "id"]`.
pub(crate) fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        if c == '_' || c == '-' || c == '.' || c == ' ' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        // A capital starts a word, unless it is inside a run of capitals that is not ending
        // (`OrderID` is order + id, `IDOrder` is id + order).
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if c.is_uppercase() && !current.is_empty() && (prev_lower || next_lower) {
            out.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Words Go capitalises whole, by convention.
const INITIALISMS: &[&str] = &[
    "id", "url", "uri", "api", "http", "https", "json", "xml", "sql", "db", "uuid", "ip", "tcp",
    "udp", "ttl", "cpu", "ram", "os", "io", "eof",
];

fn snake(w: &[String]) -> String {
    w.join("_")
}

fn kebab(w: &[String]) -> String {
    w.join("-")
}

fn screaming(w: &[String]) -> String {
    w.iter()
        .map(|s| s.to_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn camel(w: &[String]) -> String {
    w.iter()
        .enumerate()
        .map(|(i, s)| if i == 0 { s.clone() } else { capitalize(s) })
        .collect()
}

fn pascal(w: &[String]) -> String {
    w.iter().map(|s| capitalize(s)).collect()
}

/// Go's spelling: the same as Pascal case, except that an initialism is written whole.
fn pascal_go(w: &[String]) -> String {
    w.iter()
        .map(|s| {
            if INITIALISMS.contains(&s.as_str()) {
                s.to_uppercase()
            } else {
                capitalize(s)
            }
        })
        .collect()
}

/// A naming style: what it is called, and how it writes a name's words.
type Style = (&'static str, fn(&[String]) -> String);

/// Every spelling of `field`, paired with the same spelling of `to`.
pub fn variants(field: &str, to: &str) -> Vec<Variant> {
    let (f, t) = (words(field), words(to));
    let styles: [Style; 6] = [
        ("snake_case", snake),
        ("camelCase", camel),
        ("PascalCase", pascal),
        ("Go PascalCase", pascal_go),
        ("SCREAMING_CASE", screaming),
        ("kebab-case", kebab),
    ];
    let mut out: Vec<Variant> = Vec::new();
    for (style, render) in styles {
        let from = render(&f);
        let to = render(&t);
        if from.is_empty() || out.iter().any(|v| v.from == from) {
            continue;
        }
        out.push(Variant { from, to, style });
    }
    out
}
