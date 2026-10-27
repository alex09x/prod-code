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

/// A one-word field that snake_case and camelCase both spell keeps both new spellings, and an
/// occurrence takes the one its language writes (#1450).
#[test]
fn a_one_word_field_takes_each_languages_target() {
    let v = variants("status", "order_status");
    let lower = v
        .iter()
        .find(|x| x.from == "status")
        .expect("lowercase spelling");
    assert_eq!(lower.to, "order_status");
    assert_eq!(lower.target_for(Path::new("db/schema.sql")), "order_status");
    assert_eq!(lower.target_for(Path::new("src/model.rs")), "order_status");
    assert_eq!(lower.target_for(Path::new("web/order.ts")), "orderStatus");
    assert_eq!(lower.target_for(Path::new("api/order.go")), "orderStatus");
    let pascal = v
        .iter()
        .find(|x| x.from == "Status")
        .expect("Pascal spelling");
    assert_eq!(pascal.target_for(Path::new("web/order.ts")), "OrderStatus");
    // A two-word field has no shared spelling, so nothing changes for it.
    assert!(
        variants("order_id", "trade_id")
            .iter()
            .all(|x| x.alternatives.is_empty())
    );
}

/// A new name with no words is refused before anything is read or written, even with `force`.
#[tokio::test]
async fn a_target_without_words_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let sql = dir.path().join("schema.sql");
    std::fs::write(&sql, "CREATE TABLE t (status TEXT);\n").unwrap();
    let remote: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    for to in ["", "---", "_ _"] {
        let error =
            super::super::execute::rename(remote, dir.path(), "status", to, true, true, None)
                .await
                .expect_err("an empty target is an input error");
        assert!(
            error.to_string().contains("has no words"),
            "{to:?}: {error}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&sql).unwrap(),
        "CREATE TABLE t (status TEXT);\n"
    );
}
