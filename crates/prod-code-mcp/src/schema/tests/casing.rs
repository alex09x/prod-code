/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::super::casing::{variants, words};
use super::super::detect::{Kind, kind_of};
use super::super::scan::{inside_quotes, scan};

#[test]
fn splits_a_name_however_it_is_written() {
    for name in [
        "order_id", "orderId", "OrderId", "OrderID", "ORDER_ID", "order-id",
    ] {
        assert_eq!(words(name), ["order", "id"], "{name}");
    }
    assert_eq!(words("httpURLBuilder"), ["http", "url", "builder"]);
}

#[test]
fn every_language_gets_its_own_spelling() {
    let v = variants("order_id", "trade_id");
    let by_style = |style: &str| {
        v.iter()
            .find(|x| x.style == style)
            .map(|x| (x.from.as_str(), x.to.as_str()))
    };
    assert_eq!(by_style("snake_case"), Some(("order_id", "trade_id")));
    assert_eq!(by_style("camelCase"), Some(("orderId", "tradeId")));
    assert_eq!(by_style("PascalCase"), Some(("OrderId", "TradeId")));
    assert_eq!(by_style("Go PascalCase"), Some(("OrderID", "TradeID")));
    assert_eq!(by_style("SCREAMING_CASE"), Some(("ORDER_ID", "TRADE_ID")));
}

#[test]
fn a_spelling_that_repeats_is_listed_once() {
    // `symbol` is one word: snake, camel and kebab all spell it the same.
    let v = variants("symbol", "ticker");
    assert_eq!(v.iter().filter(|x| x.from == "symbol").count(), 1);
}

#[test]
fn only_whole_words_are_found() {
    let v = variants("order_id", "trade_id");
    let text = "let order_id = 1; let reorder_id = 2; let order_ident = 3;\n";
    let found = scan(text, &v, Path::new("a.rs"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].col, 5);
}

#[test]
fn a_name_inside_a_string_is_marked() {
    let v = variants("order_id", "trade_id");
    let text = "OrderID string `json:\"order_id\"`\n";
    let found = scan(text, &v, Path::new("a.go"));
    assert_eq!(found.len(), 2);
    assert!(!found.iter().any(|o| o.in_string && o.variant == 3));
    let in_tag = found.iter().find(|o| o.in_string).expect("the tag");
    assert_eq!(&v[in_tag.variant].from, "order_id");
}

#[test]
fn quotes_of_every_kind_are_understood() {
    assert!(inside_quotes("a = \"order_id\"", 7));
    assert!(inside_quotes("a = `order_id`", 7));
    assert!(!inside_quotes("a = order_id", 5));
    assert!(!inside_quotes("a = \"x\" + order_id", 12));
}

#[test]
fn a_file_is_classified_by_what_owns_it() {
    assert_eq!(kind_of(Path::new("a/b.rs")), Kind::Code("rust"));
    assert_eq!(kind_of(Path::new("a/b.proto")), Kind::Text("protobuf"));
    assert_eq!(kind_of(Path::new("a/b.png")), Kind::Skip);
}
