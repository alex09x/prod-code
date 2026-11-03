/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Extracting a `match` from a `&mut self` method that reads a local owned struct (#1812): the
//! owned value is no borrow of `self`, a field read is no read of a same-named local, and real
//! borrows of `self` are still refused. The programs are compiled and run with `rustc`.

use super::{conflicting_borrow_captures, reacquire_conflicting_borrows};
use crate::extract_function::binding::uncaptured_enclosing_locals;
use crate::extract_function::rewrite_of;

const PROGRAM: &str = r#"struct Order {
    id: u64,
    qty: u32,
    buy: bool,
}

struct Book {
    next_id: u64,
    filled: u64,
    log: Vec<String>,
    pending: Order,
}
@TYPES@
impl Book {
    fn draft(&self, qty: u32, buy: bool) -> Order {
        Order { id: self.next_id, qty, buy }
    }

    fn submit(&mut self, qty: u32, buy: bool) -> (Vec<u64>, Vec<u64>) {
        @ORDER@
        let mut buys = @BUYS@;
        let mut sells = Vec::new();
        @BODY@
        self.next_id = order.id + 1;
        (buys, sells)
    }
@HELPER@}

fn main() {
    let pending = Order { id: 0, qty: 0, buy: false };
    let mut book = Book { next_id: 7, filled: 0, log: Vec::new(), pending };
    let first = book.submit(3, true);
    let second = book.submit(2, false);
    println!("{first:?} {second:?} {} {} {:?}", book.next_id, book.filled, book.log);
}
"#;

/// The selection: the whole `match`.
const MATCH: &str = r#"match order.buy {
            true => {
                self.filled += u64::from(order.qty);
                buys.push(order.id);
            }
            false => {
                self.log.push(format!("sell {}", order.id));
                sells.push(order.id);
            }
        }"#;

const LITERAL: &str = "let order = Order {\n            id: self.next_id,\n            qty,\n            buy,\n        };";
const TYPED: &str = "let order: Order = self.draft(qty, buy);";
const EFFECTS: &str = "([7], []) ([], [8]) 9 3 [\"sell 8\"]\n";

struct Case {
    old: String,
    new: String,
    start: usize,
    end: usize,
}

fn program(types: &str, order: &str, buys: &str, body: &str, helper: &str) -> String {
    PROGRAM
        .replace("@TYPES@", types)
        .replace("@ORDER@", order)
        .replace("@BUYS@", buys)
        .replace("@BODY@", body)
        .replace("@HELPER@", helper)
}

/// The file before and after rust-analyzer's `extract_function`, in the shape it produced for
/// this selection: `self.fun_name(&order, &mut buys, &mut sells);` and the helper after `submit`.
fn case(types: &str, order: &str, buys: &str, arg: &str, param: &str) -> Case {
    let old = program(types, order, buys, MATCH, "");
    let call = format!("self.fun_name({arg}, &mut buys, &mut sells);");
    let helper = format!(
        "\n    fn fun_name(&mut self, order: {param}, buys: &mut Vec<u64>, sells: &mut Vec<u64>) {{\n        {MATCH}\n    }}\n"
    );
    let new = program(types, order, buys, &call, &helper);
    let start = old.find(MATCH).unwrap();
    Case {
        end: start + MATCH.len(),
        start,
        old,
        new,
    }
}

impl Case {
    fn span(&self, new: &str) -> Option<(usize, usize)> {
        let r = rewrite_of(&self.old, new, self.start, self.end)?;
        let at = self.start + r.call.len() + (r.inserted_at - self.end);
        Some((at, at + r.function_len))
    }

    /// The production order: re-acquire what can be, then report what still conflicts.
    fn fixed_and_conflicts(&self) -> (String, Vec<String>) {
        let fixed = reacquire_conflicting_borrows(&self.old, self.start, self.end, &self.new);
        let span = self.span(&fixed);
        assert!(span.is_some(), "the extraction must be readable: {fixed}");
        let conflicts = conflicting_borrow_captures(&self.old, self.start, self.end, &fixed, span);
        (fixed, conflicts)
    }
}

/// Compiles `source` with `rustc` and runs it: its stdout, or the compiler's errors.
fn run(source: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().unwrap();
    let (src, bin) = (dir.path().join("main.rs"), dir.path().join("main"));
    std::fs::write(&src, source).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = std::process::Command::new(rustc)
        .args(["--edition", "2021", "-A", "warnings", "-o"])
        .arg(&bin)
        .arg(&src)
        .output()
        .expect("rustc must be available to prove the extraction compiles");
    if !built.status.success() {
        return Err(String::from_utf8_lossy(&built.stderr).into_owned());
    }
    let ran = std::process::Command::new(&bin).output().unwrap();
    assert!(ran.status.success(), "{source}");
    Ok(String::from_utf8_lossy(&ran.stdout).into_owned())
}

#[test]
fn an_owned_local_struct_passed_by_reference_is_no_borrow_of_self() {
    for order in [LITERAL, TYPED] {
        let c = case("", order, "Vec::new()", "&order", "&Order");
        let (fixed, conflicts) = c.fixed_and_conflicts();
        assert_eq!(
            fixed, c.new,
            "an owned value is not re-acquired in the helper"
        );
        assert!(conflicts.is_empty(), "`{order}`: {conflicts:?}");
        let uncaptured = uncaptured_enclosing_locals(&c.old, c.start, &fixed, c.span(&fixed));
        assert!(
            uncaptured.is_empty(),
            "`order.qty` is no read of `qty`: {uncaptured:?}"
        );
        assert_eq!(run(&c.old).as_deref(), Ok(EFFECTS), "the original runs");
        assert_eq!(
            run(&fixed).as_deref(),
            Ok(EFFECTS),
            "the extraction compiles and keeps the effects on self, buys and sells"
        );
    }
}

#[test]
fn a_bare_read_of_an_enclosing_local_is_still_uncaptured() {
    let c = case("", LITERAL, "Vec::new()", "&order", "&Order");
    let new = c.new.replacen("u64::from(order.qty)", "u64::from(qty)", 1);
    assert_eq!(
        uncaptured_enclosing_locals(&c.old, c.start, &new, c.span(&new)),
        vec!["qty".to_string()]
    );
}

#[test]
fn a_real_borrow_of_self_is_still_refused_and_does_not_compile() {
    let c = case(
        "",
        "let order = &self.pending;",
        "Vec::new()",
        "order",
        "&Order",
    );
    let (fixed, conflicts) = c.fixed_and_conflicts();
    assert_eq!(
        fixed, c.new,
        "a borrow read after the call is not re-acquired"
    );
    assert!(
        conflicts.iter().any(|e| e.contains("via `order`")),
        "{conflicts:?}"
    );
    assert!(
        run(&c.old).is_ok(),
        "disjoint field borrows compile in place"
    );
    let error = run(&fixed).expect_err("the refused extraction must not compile");
    assert!(error.contains("E0502"), "{error}");
}

#[test]
fn unproven_owners_borrow_lifetimes_and_collections_stay_refused() {
    let lifetime = "\nstruct View<'a> {\n    id: &'a u64,\n}\n";
    let view = case(
        lifetime,
        "let order = View { id: &self.next_id };",
        "Vec::new()",
        "&order",
        "&View",
    );
    let ambiguous = "\nmod other {\n    pub struct Order {\n        pub id: u64,\n    }\n}\n";
    let shadowed = case(ambiguous, LITERAL, "Vec::new()", "&order", "&Order");
    for (what, c) in [
        ("a struct with a lifetime", view),
        ("an ambiguous name", shadowed),
    ] {
        let (_, conflicts) = c.fixed_and_conflicts();
        assert!(
            conflicts.iter().any(|e| e.contains("via `order`")),
            "{what}: {conflicts:?}"
        );
    }
    let collection = case(
        "",
        LITERAL,
        "Vec::with_capacity(self.log.len())",
        "&order",
        "&Order",
    );
    let (_, conflicts) = collection.fixed_and_conflicts();
    assert!(
        conflicts.iter().any(|e| e.contains("via `buys`"))
            && !conflicts.iter().any(|e| e.contains("via `order`")),
        "{conflicts:?}"
    );
}
