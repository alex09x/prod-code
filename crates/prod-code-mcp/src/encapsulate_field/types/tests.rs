/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

#[test]
fn coverage_accessor_reports_across_languages() {
    let mut result = EncapsulatedField {
        owner: "Item".into(), root: PathBuf::new(), file: String::new(), field: "count".into(),
        ty: "int".into(), by_value: true, reads: 2, writes: 1, chained_reads: 0,
        left_in_file: 3, blocked: vec![], unmatched: vec![], rewritten: vec![],
        diagnostics: vec![], applied: false,
    };
    for (file, by_value, owner, getter, setter) in [
        ("item.ts", true, "Item.count", "getCount(): int", "setCount(count: int): void"),
        ("item.tsx", true, "Item.count", "getCount(): int", "setCount(count: int): void"),
        ("item.js", true, "Item.count", "getCount(): int", "setCount(count: int): void"),
        ("item.jsx", true, "Item.count", "getCount(): int", "setCount(count: int): void"),
        ("item.py", true, "Item.count", "def get_count(self) -> int", "def set_count(self, count: int) -> None"),
        ("item.cpp", true, "Item::count", "int get_count() const", "void set_count(int count)"),
        ("item.hpp", false, "Item::count", "const int& get_count() const", "void set_count(const int& count)"),
        ("item.swift", true, "Item.count", "func getCount() -> int", "func setCount(_: int)"),
        ("item.go", true, "Item.count", "func (s *Item) Count() int", "func (s *Item) SetCount(count int)"),
        ("item.rs", true, "Item.count", "fn count(&self) -> int", "fn set_count(&mut self, count: int)"),
        ("item.rs", false, "Item.count", "fn count(&self) -> &int", "fn set_count(&mut self, count: int)"),
    ] {
        result.file = file.into(); result.by_value = by_value;
        for writes in [0, 1] {
            result.writes = writes;
            let setter_line = if writes == 1 { format!("- setter: `{setter}`\n") } else { String::new() };
            assert_eq!(result.render(1000), format!("`{owner}` ({file})\n\n- the field becomes private\n- getter: `{getter}`\n{setter_line}- 2 read(s) and {writes} write(s) outside {file} rewritten; 3 reference(s) inside it left as they are, because a private field is still visible there\n\n0 changed line(s) in 0 file(s)\n\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make these edits\n"));
        }
    }
}
