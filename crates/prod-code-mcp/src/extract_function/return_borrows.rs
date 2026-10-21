/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;

use anyhow::Result;

use super::{PLACEHOLDER, tokens::tokens};

/// The assist chooses the returned local's inferred type, which may contain an elided
/// borrow even though the new helper has several possible input lifetimes. Analyzer
/// diagnostics do not reliably include E0106, so enforce elision rules before proposing
/// the rewrite. Choosing an owner or an input lifetime requires provenance we do not have.
pub(super) fn check_return_borrows(extracted: &str) -> Result<()> {
    let Some(def) = extracted.find(&format!("fn {PLACEHOLDER}")) else {
        return Ok(());
    };
    let Some((_, open, close)) = crate::signature::param_span(extracted, def + 3) else {
        return Ok(());
    };
    let Some(body) = extracted[close..].find('{').map(|i| close + i) else {
        return Ok(());
    };
    let suffix = &extracted[close..body];
    let where_at = tokens(suffix)
        .into_iter()
        .find(|(_, s, e)| &suffix[*s..*e] == "where")
        .map_or(suffix.len(), |(_, s, _)| s);
    let suffix = &suffix[..where_at];
    let Some((_, output)) = suffix.split_once("->") else {
        return Ok(());
    };
    if !has_elided_borrow(output) {
        return Ok(());
    }

    let params = &extracted[open..close];
    let (receiver, _) = crate::signature::parse_declared(params);
    // Method lifetime elision assigns every elided output borrow to &self.
    if receiver.as_deref().is_some_and(|r| {
        r.trim_start().starts_with('&')
            || r.split_once(':')
                .is_some_and(|(_, ty)| ty.trim_start().starts_with('&'))
    }) {
        return Ok(());
    }
    let mut lifetimes = BTreeSet::new();
    let ts = tokens(params);
    for (i, (_, start, end)) in ts.iter().enumerate() {
        let word = &params[*start..*end];
        if word == "&"
            && !ts
                .get(i + 1)
                .is_some_and(|(_, s, e)| params[*s..*e].starts_with('\''))
        {
            lifetimes.insert(format!("elided-{i}"));
        } else if word.starts_with('\'') {
            lifetimes.insert(if word == "'_" {
                format!("elided-{i}")
            } else {
                word.to_string()
            });
        }
    }
    anyhow::ensure!(
        lifetimes.len() == 1,
        "cannot extract a borrowed value with an elided return lifetime: the helper has {} input lifetimes; \
         select a smaller region that takes the borrowed value as an input, \
         or extend the selection to consume it so it need not be returned. An explicit lifetime tied \
         to the correct input or an owned return value is required; nothing was written",
        lifetimes.len()
    );
    Ok(())
}

fn has_elided_borrow(ty: &str) -> bool {
    let ts = tokens(ty);
    ts.iter().enumerate().any(|(i, (_, start, end))| {
        let word = &ty[*start..*end];
        word == "'_"
            || word == "&"
                && !ts.get(i + 1).is_some_and(|(_, s, e)| {
                    let lifetime = &ty[*s..*e];
                    lifetime.starts_with('\'') && lifetime != "'_"
                })
    })
}

#[cfg(test)]
mod tests {
    use super::check_return_borrows;

    #[test]
    fn extract_function_return_lifetime_elision_rules() {
        for signature in [
            "fn fun_name(a: &str, b: &str) -> &str { a }",
            "fn fun_name(a: (&str, &str)) -> Option<&str> { None }",
            "fn fun_name(a: &str, b: &str) -> Thing<'_> { todo!() }",
            "fn fun_name(a: &Thing<'_>) -> &str { todo!() }",
            "fn fun_name() -> &str { todo!() }",
        ] {
            assert!(check_return_borrows(signature).is_err(), "{signature}");
        }
        for signature in [
            "fn fun_name(a: &str) -> &str { a }",
            "fn fun_name(a: &str, b: &str) -> String { a.to_owned() }",
            "fn fun_name<'a>(a: &'a str, b: &str) -> &'a str { a }",
            "fn fun_name<'a>(a: &'a str, b: &'a str) -> &str { a }",
            "fn fun_name(a: &str, b: &str) -> &'static str { \"ok\" }",
            "fn fun_name(&self, b: &str) -> &str { self.name }",
            "fn fun_name(self: &Self, b: &str) -> &str { self.name }",
        ] {
            assert!(check_return_borrows(signature).is_ok(), "{signature}");
        }
    }
}
