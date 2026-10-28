/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeSet, HashMap};

use crate::diagnostics::annotate::annotate_missing_symbols;
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

#[test]
fn non_rust_notes_use_unresolved_code_tokens_and_state_uncertainty() {
    let file = "main.go".to_string();
    let source = "package main\nfunc run() { B.new() } // new\nvar s = \"new\" // new\nfunc other() { new() }\n";
    let diagnostic = |line| DocDiagnostic {
        severity: "warning".into(),
        code: None,
        message: "independent warning".into(),
        line,
        col: 1,
        source: None,
        note: None,
        end: None,
    };
    let mut reports = vec![DiagnosticsReport {
        file: file.clone(),
        errors: 0,
        warnings: 3,
        items: vec![diagnostic(2), diagnostic(3), diagnostic(4)],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let sources = HashMap::from([(file.clone(), source.to_string())]);
    let resolved = BTreeSet::from([(file, 2, 16)]);
    let missing = vec![("new".to_string(), "owner.go".to_string())];
    annotate_missing_symbols(&mut reports, &sources, &missing, &resolved);
    assert!(reports[0].items[0].note.is_none());
    assert!(reports[0].items[1].note.is_none());
    let note = reports[0].items[2].note.as_deref().unwrap();
    assert!(note.contains("may"), "{note}");
}
