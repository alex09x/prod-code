/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::evidence::parameter_evidence;
use crate::signature_go::hazards::is_literal;
use crate::signature_go::interface::declares_interface_method;
use crate::signature_go::syntax::scalar_literal;
use std::path::Path;

#[cfg(test)]
mod removal_evidence_validation_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_parameter_locations_cannot_prove_a_removal_safe() {
        let file = Path::new("/tmp/parameter-proof.go");
        let text = "func F(discard int) {}";
        let at = text.find("discard").unwrap();
        let body = (text.find('{').unwrap(), text.find('}').unwrap());
        let good = json!({"uri":url::Url::from_file_path(file).unwrap().to_string(),
            "range":{"start":{"line":0,"character":at},"end":{"line":0,"character":at+7}}});
        assert!(
            parameter_evidence(&json!([good.clone()]), file, text, "discard", at, body)
                .unwrap()
                .is_empty()
        );
        let mut no_end = good.clone();
        no_end["range"].as_object_mut().unwrap().remove("end");
        let mut wrong_end = good.clone();
        wrong_end["range"]["end"]["character"] = json!(at);
        let mut raw_path = good.clone();
        raw_path["uri"] = json!(file.to_str().unwrap());
        let mut overflow = good;
        overflow["range"]["start"]["line"] = json!(u32::MAX);
        overflow["range"]["end"]["line"] = json!(u32::MAX);
        let mut failures = Vec::new();
        for entry in [no_end, wrong_end, raw_path, overflow] {
            let result = std::panic::catch_unwind(|| {
                parameter_evidence(&json!([entry.clone()]), file, text, "discard", at, body)
            });
            if !matches!(result, Ok(Err(_))) {
                failures.push(format!("accepted or panicked: {entry}: {result:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

#[cfg(test)]
mod literal_expression_regression {
    #[test]
    fn hexadecimal_digits_do_not_introduce_decimal_exponent_signs() {
        for literal in [
            "0x1e", "-0X1E", "0x1p+2", "0X1P-2", "1e+2", "-1E-2", "0x1p+2i",
        ] {
            assert!(super::is_literal(literal), "{literal}");
        }
        for expression in ["0x1e+2", "0x1E-2", "0x1e+counter", "0X1E-value"] {
            assert!(!super::is_literal(expression), "{expression}");
            assert!(!super::scalar_literal(expression), "{expression}");
        }
    }
}

#[cfg(test)]
mod interface_name_boundary_primary_probe {
    use super::*;
    #[test]
    fn an_unrelated_interface_member_does_not_block_receiver_addition() {
        assert!(
            !declares_interface_method("type Unrelated interface { NotAdd(x int) string }", "Add"),
            "NotAdd is not Add"
        );
        assert!(!declares_interface_method(
            "type Unrelated interface { Other(Add (int)) }",
            "Add"
        ));
        assert!(
            !declares_interface_method("const note = `interface { Add(int) string }`", "Add"),
            "source text in a string is not an interface obligation"
        );
        assert!(
            declares_interface_method(
                "type Related interface { Add /* comment */ (int) string }",
                "Add"
            ),
            "whitespace/comments are allowed before the parameter list"
        );
    }
}
