/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[cfg(test)]
mod tests {
    use super::super::lexer::{Kind, Source, lex};
    use super::super::outline::{one_based, outline_node, range_lines};
    use super::super::plan::plan;
    use super::super::types::{BuilderPlan, BuilderPreview, OutlineNode, Verification};
    use super::super::verify::locations;

    const CONFIG: &str = "use std::collections::HashMap;

/// Settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// The name.
    pub name: String,
    pub(crate) r#type: u8,
    limits: HashMap<String, Vec<(u8, Option<Box<[u16; 4]>>)>>,
    callback: fn(&str) -> Result<u8, String>,
    nested:
        Vec<
            Vec<u8>,
        >,
}

fn other() {}
";

    fn config_plan() -> BuilderPlan {
        plan(CONFIG, "Config", (3, 15), None).expect("a plan")
    }

    #[test]
    fn a_raw_struct_name_and_crlf_line_endings_are_kept() {
        let text = "pub struct r#Match {\r\n    pub r#in: u8,\r\n}\r\n";
        let plan = plan(text, "Match", (1, 3), None).unwrap();
        assert_eq!(plan.builder_name, "MatchBuilder");
        assert!(plan.code.contains("Result<r#Match, MatchBuilderError>"));
        assert!(
            plan.code
                .contains("::core::result::Result::Ok(r#Match {\r\n")
        );
        assert!(!plan.file_text.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn the_lexer_reads_literals_lifetimes_and_comments() {
        let text = "a /* x /* y */ z */ 'b' b'\\'' '\\u{1F600}' 'static r#\"q\"# br##\"w\"## c\"e\" 1.5e3 -> :: // tail\n";
        let src = Source::new(text).unwrap();
        let kinds: Vec<(Kind, &str)> = (0..src.tokens.len())
            .map(|i| (src.tokens[i].kind, src.t(i)))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (Kind::Ident, "a"),
                (Kind::Literal, "'b'"),
                (Kind::Literal, "b'\\''"),
                (Kind::Literal, "'\\u{1F600}'"),
                (Kind::Lifetime, "'static"),
                (Kind::Literal, "r#\"q\"#"),
                (Kind::Literal, "br##\"w\"##"),
                (Kind::Literal, "c\"e\""),
                (Kind::Literal, "1.5e3"),
                (Kind::Punct, "->"),
                (Kind::Punct, "::"),
            ]
        );
        assert!(lex("/* open").is_err());
        assert!(lex("\"open").is_err());
    }

    #[test]
    fn the_outline_node_is_the_struct_not_a_field_on_its_line() {
        let symbols = serde_json::json!([{
            "name": "P", "kind": 23,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 30 } },
            "children": [
                { "name": "r#a", "kind": 8,
                  "range": { "start": { "line": 0, "character": 15 }, "end": { "line": 0, "character": 20 } } }
            ]
        }]);
        assert_eq!(
            outline_node(&symbols, "P", 1).unwrap(),
            Some(OutlineNode {
                start: 1,
                end: 1,
                fields: Some(vec!["r#a".into()])
            })
        );
        assert_eq!(outline_node(&symbols, "Q", 1).unwrap(), None);
        assert_eq!(outline_node(&symbols, "P", 2).unwrap(), None);
        assert_eq!(
            outline_node(&serde_json::Value::Null, "P", 1).unwrap(),
            None
        );

        // The Rust engine's flat answer: a field is named by its container path and its line.
        let at = |line: u64| serde_json::json!({ "start": { "line": line, "character": 4 }, "end": { "line": line, "character": 0 } });
        let flat = serde_json::json!([
            { "name": "network", "kind": 2, "location": { "uri": "file:///w/a.rs", "range": at(0) } },
            { "name": "P", "kind": 23, "containerName": "network", "location": { "uri": "file:///w/a.rs", "range": { "start": { "line": 2, "character": 15 }, "end": { "line": 5, "character": 0 } } } },
            { "name": "r#type", "kind": 8, "containerName": "network > P", "location": { "uri": "file:///w/a.rs", "range": at(3) } },
            { "name": "b", "kind": 8, "containerName": "network > P", "location": { "uri": "file:///w/a.rs", "range": at(4) } },
            { "name": "c", "kind": 8, "containerName": "network > Q", "location": { "uri": "file:///w/a.rs", "range": at(8) } },
        ]);
        assert_eq!(
            outline_node(&flat, "P", 3).unwrap(),
            Some(OutlineNode {
                start: 3,
                end: 6,
                fields: Some(vec!["r#type".into(), "b".into()])
            })
        );
    }

    /// A range over the 0-based lines `from..=to`, written as JSON.
    fn span(from: serde_json::Value, to: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "start": { "line": from, "character": 0 }, "end": { "line": to, "character": 1 } })
    }

    #[test]
    fn coordinates_are_converted_checked() {
        let n = |v: serde_json::Value| one_based(Some(&v));
        assert_eq!(n(serde_json::json!(0)), Some(1));
        assert_eq!(n(serde_json::json!(u32::MAX - 1)), Some(u32::MAX));
        assert_eq!(n(serde_json::json!(u32::MAX)), None);
        assert_eq!(n(serde_json::json!(1u64 << 32)), None);
        assert_eq!(n(serde_json::json!(-1)), None);
        assert_eq!(n(serde_json::json!(1.5)), None);
        assert_eq!(n(serde_json::json!("3")), None);
        assert_eq!(one_based(None), None);
        assert_eq!(
            range_lines(&span(serde_json::json!(2), serde_json::json!(4))),
            Some((3, 5))
        );
        // Zero width, as sourcekit-lsp answers, is a range.
        let point = serde_json::json!({ "line": 2, "character": 3 });
        assert_eq!(
            range_lines(&serde_json::json!({ "start": point, "end": point })),
            Some((3, 3))
        );
        // A field in the Rust engine's flat outline: it ends at character 0 of the line it
        // starts on, which rust-analyzer behind the gateway really answers.
        assert_eq!(
            range_lines(
                &serde_json::json!({ "start": { "line": 27, "character": 8 }, "end": { "line": 27, "character": 0 } })
            ),
            Some((28, 28))
        );
        for bad in [
            span(serde_json::json!(4), serde_json::json!(2)),
            serde_json::json!({ "start": { "line": 2, "character": -5 }, "end": { "line": 2, "character": 1 } }),
            serde_json::json!({ "start": { "line": 2, "character": 0 }, "end": { "line": 2, "character": u32::MAX } }),
            serde_json::json!({ "start": { "line": 2 }, "end": { "line": 2, "character": 1 } }),
            serde_json::json!({ "start": { "line": 2, "character": 0 } }),
            span(serde_json::json!(u32::MAX), serde_json::json!(u32::MAX)),
            serde_json::json!(null),
        ] {
            assert_eq!(range_lines(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_malformed_outline_is_an_error_not_a_panic_or_a_guess() {
        let node = |range: serde_json::Value| serde_json::json!([{ "name": "P", "kind": 23, "range": range }]);
        let past_u32 = 1u64 << 32;
        for (outline, expected) in [
            (
                node(span(serde_json::json!(0), serde_json::json!(u32::MAX))),
                "a range that is not one",
            ),
            (
                node(span(
                    serde_json::json!(past_u32),
                    serde_json::json!(past_u32 + 2),
                )),
                "a range that is not one",
            ),
            (
                node(span(serde_json::json!(-1), serde_json::json!(2))),
                "a range that is not one",
            ),
            (
                serde_json::json!([{ "name": "P", "kind": 23 }]),
                "a range that is not one: none",
            ),
            (
                serde_json::json!([{ "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)),
                    "children": [{ "kind": 8, "range": span(serde_json::json!(1), serde_json::json!(1)) }] }]),
                "a field of `P` without a name",
            ),
            (
                serde_json::json!([{ "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)),
                    "children": [{ "name": "a" }] }]),
                "a member of `P` without a kind",
            ),
            (
                serde_json::json!([{ "name": "m", "kind": 2, "range": span(serde_json::json!(0), serde_json::json!(9)),
                    "children": "P" }]),
                "members that are not a list",
            ),
            (
                serde_json::json!([
                    { "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)) },
                    { "name": "a", "kind": 8, "containerName": "P",
                      "range": span(serde_json::json!(past_u32 + 1), serde_json::json!(past_u32 + 1)) },
                ]),
                "field `a` of `P` a range that is not one",
            ),
            (
                serde_json::json!([
                    { "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)) },
                    { "kind": 8, "containerName": "P", "range": span(serde_json::json!(1), serde_json::json!(1)) },
                ]),
                "a field of `P` without a name",
            ),
            (serde_json::json!({ "name": "P" }), "not a list of symbols"),
        ] {
            let err = outline_node(&outline, "P", 1).expect_err(&outline.to_string());
            let err = format!("{err:#}");
            assert!(err.contains(expected), "{outline}\n=> {err}");
        }
        // What is not about `P` does not stop it: another name's range, a field of another
        // struct, members of another kind, `null` children.
        let fine = serde_json::json!([
            { "name": "Q", "kind": 23, "range": span(serde_json::json!(u32::MAX), serde_json::json!(-1)) },
            { "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)),
              "children": [
                  { "name": "a", "kind": 8, "range": span(serde_json::json!(1), serde_json::json!(1)) },
                  { "name": "f", "kind": 6, "range": span(serde_json::json!(1), serde_json::json!(1)), "children": null },
              ] },
            { "name": "b", "kind": 8, "containerName": "Q", "range": span(serde_json::json!(-1), serde_json::json!(-1)) },
        ]);
        assert_eq!(
            outline_node(&fine, "P", 2).unwrap(),
            Some(OutlineNode {
                start: 1,
                end: 3,
                fields: Some(vec!["a".into()])
            })
        );
    }

    #[test]
    fn definition_answers_of_every_shape_are_read() {
        let link = serde_json::json!([{ "targetUri": "file:///w/src/lib.rs",
            "targetRange": { "start": { "line": 1, "character": 0 }, "end": { "line": 3, "character": 1 } },
            "targetSelectionRange": { "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 5 } } }]);
        assert_eq!(
            locations(&link).unwrap(),
            vec![(std::path::PathBuf::from("/w/src/lib.rs"), 3)]
        );
        let link_without_selection = serde_json::json!([{ "targetUri": "file:///w/src/lib.rs",
            "targetRange": { "start": { "line": 1, "character": 0 }, "end": { "line": 3, "character": 1 } } }]);
        assert_eq!(
            locations(&link_without_selection).unwrap(),
            vec![(std::path::PathBuf::from("/w/src/lib.rs"), 2)]
        );
        let single = serde_json::json!({ "uri": "file:///w/a.rs",
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } } });
        assert_eq!(locations(&single).unwrap().len(), 1);
        assert!(locations(&serde_json::Value::Null).unwrap().is_empty());
        assert!(locations(&serde_json::json!([])).unwrap().is_empty());
    }

    #[test]
    fn a_malformed_definition_answer_is_an_error_not_an_empty_one() {
        let range = span(serde_json::json!(8), serde_json::json!(8));
        let uri = "file:///w/a.rs";
        for answer in [
            serde_json::json!([{ "uri": uri }]),
            serde_json::json!([{ "range": range }]),
            serde_json::json!([{ "uri": 7, "range": range }]),
            serde_json::json!([{ "uri": "not a uri", "range": range }]),
            serde_json::json!([{ "uri": "untitled:Untitled-1", "range": range }]),
            serde_json::json!([{ "targetUri": uri, "range": range }]),
            serde_json::json!([{ "targetUri": uri, "targetSelectionRange": span(serde_json::json!(u32::MAX), serde_json::json!(u32::MAX)) }]),
            serde_json::json!([{ "uri": uri, "range": span(serde_json::json!(1u64 << 32), serde_json::json!(1u64 << 32)) }]),
            serde_json::json!([{ "uri": uri, "range": span(serde_json::json!(-1), serde_json::json!(0)) }]),
            // One good location does not make up for a bad one next to it.
            serde_json::json!([{ "uri": uri, "range": range }, { "uri": uri }]),
            serde_json::json!([null]),
            serde_json::json!("P"),
            serde_json::json!(true),
        ] {
            let err = locations(&answer).expect_err(&answer.to_string());
            assert!(
                format!("{err:#}").contains("malformed definition answer"),
                "{err:#}"
            );
        }
    }

    #[test]
    fn the_report_tells_verified_from_unverified() {
        let mut preview = BuilderPreview {
            file: "src/lib.rs".into(),
            plan: config_plan(),
            verification: Verification::Clean,
        };
        assert!(preview.verified());
        assert!(
            preview
                .render()
                .contains("verified: the analyzer checked it")
        );
        assert!(preview.render().contains("nothing was written"));
        preview.verification = Verification::Rejected {
            diagnostics: vec!["mismatched types [E0308] (src/lib.rs:20:5)".into()],
        };
        assert!(!preview.verified());
        assert_eq!(preview.diagnostics().len(), 1);
        assert!(preview.render().contains("rejected:"));
        preview.verification = Verification::Unverified {
            reason: "not asked".into(),
        };
        assert!(!preview.verified());
        assert!(preview.diagnostics().is_empty());
        assert!(preview.render().contains("not verified: not asked"));
    }
}
