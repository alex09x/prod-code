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

pub(super) fn assert_pulled(root: &Path, pulled: &[String], relaid: &[String], kept: &[String]) {
    let mut pulled = pulled.to_vec();
    pulled.sort();
    assert_eq!(pulled, ["fmt.rs", "gone.txt", "new/created.txt"]);
    assert!(
        relaid.is_empty(),
        "import reordering is not a proven whitespace rewrite"
    );
    assert_eq!(kept, ["edit.rs"]);
    let read = |name: &str| std::fs::read_to_string(root.join(name)).ok();
    assert_eq!(read("same.txt").as_deref(), Some("same\n"));
    assert_eq!(read("fmt.rs").as_deref(), Some("use a::A;\nuse b::B;\n"));
    assert_eq!(
        read("edit.rs").as_deref(),
        Some("local edit\n"),
        "a local edit is kept"
    );
    assert_eq!(read("gone.txt"), None, "a deletion is applied");
    assert_eq!(read("new/created.txt").as_deref(), Some("created\n"));
    let outside = root.parent().unwrap().join("runner-escape-probe.txt");
    assert!(
        !outside.exists(),
        "the node never writes outside the checkout"
    );
}
