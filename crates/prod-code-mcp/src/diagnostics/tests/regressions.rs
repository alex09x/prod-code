/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::json;
use std::path::Path;

use crate::diagnostics::ident::{mentions_identifier, rust_code_identifiers};
use crate::diagnostics::parse::{
    classify_hallucination, extract_method_name, is_borrow_checker_error_code,
};
use crate::diagnostics::reexport::has_definition;
use crate::diagnostics::types::{
    DiagnosticsReport, DocDiagnostic, HallucinationInterception, HallucinationKind,
    StreamValidationResult,
};
use crate::diagnostics::validate::validate_texts;

mod identifier_boundary_regressions {
    use super::*;

    #[test]
    fn combining_marks_remain_in_the_identifier() {
        let tokens =
            rust_code_identifiers("fn run() { covers\u{0301}(); r#covers\u{0301}(); covers(); }");
        let names: Vec<_> = tokens.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["fn", "run", "covers\u{0301}", "covers\u{0301}", "covers"]
        );
        assert!(!mentions_identifier("covers()", ""));
    }
}

mod definition_evidence_tests {
    use super::*;

    #[test]
    fn only_complete_definition_locations_prove_resolution() {
        let range = json!({"start":{"line":0,"character":7},"end":{"line":0,"character":13}});
        let location = json!({"uri":"file:///tmp/decl.rs","range":range});
        assert!(has_definition(&location));
        assert!(has_definition(&json!([location])));
        assert!(has_definition(
            &json!({"targetUri":"file:///tmp/decl.rs","targetSelectionRange":range})
        ));
        for value in [
            json!(null),
            json!([]),
            json!({}),
            json!([location, null]),
            json!({"uri":"file:///tmp/decl.rs","range":range,"error":{"code":-1}}),
            json!({"uri":"relative.rs","range":range}),
            json!({"uri":"file:///tmp/decl.rs"}),
            json!({"uri":"file:///tmp/decl.rs","range":{"start":{"line":1,"character":1},"end":{"line":0,"character":1}}}),
            json!({"uri":"file:///tmp/decl.rs","range":{"start":{"line":0,"character":4294967296u64},"end":{"line":0,"character":4294967296u64}}}),
        ] {
            assert!(!has_definition(&value), "{value}");
        }
    }

    #[tokio::test]
    async fn validate_texts_fails_closed_on_unreadable_also_check() {
        let root = Path::new("/nonexistent");
        let remote = "127.0.0.1:9400".parse().unwrap();
        let edits = vec![(std::path::PathBuf::from("valid.json"), "{}".to_string())];
        let also_check = vec![std::path::PathBuf::from("missing.json")];
        let result = validate_texts(remote, root, &edits, &also_check).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cannot read"), "unexpected error: {err}");
    }
}

mod hallucination_interception_phase77_tests {
    use super::*;

    #[test]
    fn borrow_checker_error_codes_identified() {
        for code in &[
            "E0382", "E0499", "E0502", "E0503", "E0505", "E0506", "E0507", "E0515", "E0521",
            "E0596", "E0597", "E0716",
        ] {
            assert!(
                is_borrow_checker_error_code(code),
                "expected {code} to be borrow-checker error"
            );
        }
        for non_borrow in &["E0308", "E0425", "E0061", "E0277", "syntax-error"] {
            assert!(
                !is_borrow_checker_error_code(non_borrow),
                "expected {non_borrow} not to be borrow-checker error"
            );
        }
    }

    #[test]
    fn polyglot_method_name_extraction() {
        assert_eq!(
            extract_method_name("no method named `stream_tokens` found for struct `Session`"),
            Some("stream_tokens".to_string())
        );
        assert_eq!(
            extract_method_name("Property 'executeAsync' does not exist on type 'Worker'"),
            Some("executeAsync".to_string())
        );
        assert_eq!(
            extract_method_name(
                "client.FetchBatch undefined (type Client has no field or method FetchBatch)"
            ),
            Some("FetchBatch".to_string())
        );
        assert_eq!(
            extract_method_name("'Manager' object has no attribute 'dispatch_event'"),
            Some("dispatch_event".to_string())
        );
        assert_eq!(
            extract_method_name("no member named 'compute_digest' in 'HashBuilder'"),
            Some("compute_digest".to_string())
        );
    }

    #[test]
    fn hallucination_classification_rules() {
        let borrow_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("E0502".to_string()),
            message: "cannot borrow `cache` as mutable because it is also borrowed as immutable"
                .to_string(),
            line: 42,
            col: 10,
            source: Some("rustc".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&borrow_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::BorrowCheckerError);
        assert!(intercept.suggestion.unwrap().contains("borrow"));

        let method_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("unresolved-method".to_string()),
            message: "no method named `nonexistent_api` found for struct `Client`".to_string(),
            line: 15,
            col: 8,
            source: Some("rust-analyzer".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&method_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::InvalidMethodInvocation);
        assert_eq!(
            intercept.symbol_or_target.as_deref(),
            Some("nonexistent_api")
        );

        let type_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("E0308".to_string()),
            message: "mismatched types: expected `u64`, found `&str`".to_string(),
            line: 25,
            col: 12,
            source: Some("rustc".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&type_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::IncorrectArgumentType);

        let syntax_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("syntax-error".to_string()),
            message: "expected `;`, found `}`".to_string(),
            line: 30,
            col: 1,
            source: Some("rust-analyzer".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&syntax_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::SyntaxError);
    }

    #[test]
    fn diagnostics_report_and_stream_result_render() {
        let intercept = HallucinationInterception {
            kind: HallucinationKind::InvalidMethodInvocation,
            symbol_or_target: Some("hallucinated_fn".to_string()),
            message: "method `hallucinated_fn` does not exist".to_string(),
            line: 12,
            col: 5,
            suggestion: Some("Check symbol declarations via code_definition".to_string()),
        };
        let report = DiagnosticsReport {
            file: "src/engine.rs".to_string(),
            errors: 1,
            warnings: 0,
            items: vec![DocDiagnostic {
                severity: "error".to_string(),
                code: Some("unresolved-method".to_string()),
                message: "method `hallucinated_fn` does not exist".to_string(),
                line: 12,
                col: 5,
                source: Some("rust-analyzer".to_string()),
                note: None,
                end: None,
            }],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![intercept.clone()],
        };

        let rendered = report.render();
        assert!(rendered.contains("=== Intercepted Hallucinations (Phase 7.7) ==="));
        assert!(rendered.contains("[INTERCEPT] InvalidMethodInvocation:"));
        assert!(rendered.contains("hallucinated_fn"));
        assert!(rendered.contains("Check symbol declarations"));

        let stream_res = StreamValidationResult {
            completed_chunks: 3,
            total_chunks: 5,
            intercepted: true,
            interception: Some(intercept),
            intercept_chunk_index: Some(2),
            final_report: Some(report),
            summary: "intercepted InvalidMethodInvocation at chunk 3".to_string(),
        };

        let stream_rendered = stream_res.render();
        assert!(stream_rendered.contains("Streamed validation:"));
        assert!(stream_rendered.contains("[INTERCEPT at chunk 3]:"));
        assert!(stream_rendered.contains("--> Check symbol declarations"));

        // Verify JSON serialization roundtrip
        let serialized = serde_json::to_string(&stream_res).unwrap();
        let deserialized: StreamValidationResult = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.completed_chunks, 3);
        assert_eq!(deserialized.intercepted, true);
    }
}
