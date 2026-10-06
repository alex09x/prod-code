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
    use super::super::plan::plan;
    use super::super::types::BuilderPlan;

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
    fn fields_keep_their_names_and_full_type_spellings() {
        let plan = config_plan();
        let fields: Vec<(&str, &str)> = plan
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.ty.as_str()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("name", "String"),
                ("r#type", "u8"),
                (
                    "limits",
                    "HashMap<String, Vec<(u8, Option<Box<[u16; 4]>>)>>"
                ),
                ("callback", "fn(&str) -> Result<u8, String>"),
                ("nested", "Vec< Vec<u8>, >"),
            ]
        );
        assert_eq!(plan.builder_name, "ConfigBuilder");
        assert_eq!(plan.error_name, "ConfigBuilderError");
        assert_eq!(plan.visibility, "pub");
        assert_eq!(plan.declaration_lines, (4, 15));
        assert_eq!(plan.insert_after_line, 15);
    }

    #[test]
    fn the_code_has_one_typed_setter_per_field_and_required_slots() {
        let plan = config_plan();
        let code = &plan.code;
        assert!(
            code.contains("    r#type: ::core::option::Option<u8>,"),
            "{code}"
        );
        assert!(
            code.contains("    pub fn r#type(mut self, value: u8) -> Self {"),
            "{code}"
        );
        assert!(
            code.contains("    pub fn limits(mut self, value: HashMap<String, Vec<(u8, Option<Box<[u16; 4]>>)>>) -> Self {"),
            "{code}"
        );
        assert!(
            code.contains(
                "            r#type: self.r#type.ok_or(ConfigBuilderError { field: \"type\" })?,"
            ),
            "{code}"
        );
        assert!(
            code.contains(
                "    pub fn build(self) -> ::core::result::Result<Config, ConfigBuilderError> {"
            ),
            "{code}"
        );
        assert!(!code.contains("Default::default"), "{code}");
        assert_eq!(code.matches("(mut self, value:").count(), 5, "{code}");
    }

    #[test]
    fn the_builder_goes_after_the_declaration_and_the_rest_is_untouched() {
        let plan = config_plan();
        let (before, after) = CONFIG.split_at(CONFIG.find("\nfn other").unwrap() + 1);
        assert!(plan.file_text.starts_with(before), "{}", plan.file_text);
        assert!(plan.file_text.ends_with(after), "{}", plan.file_text);
        let lines: Vec<&str> = plan.file_text.lines().collect();
        assert_eq!(lines[plan.insert_after_line as usize - 1], "}");
        assert_eq!(lines[plan.insert_after_line as usize], "");
        assert_eq!(
            lines[plan.code_lines.0 as usize - 1],
            "/// Builds a `Config` one field at a time. Every field is required: `ConfigBuilder::build`"
        );
        assert_eq!(
            lines[plan.code_lines.1 as usize - 1],
            "impl ::core::error::Error for ConfigBuilderError {}"
        );
    }

    #[test]
    fn a_declaration_in_a_module_is_indented_like_it() {
        let text = "mod inner {\n    pub(super) struct Point {\n        pub x: i32,\n    }\n}\n";
        let plan = plan(text, "Point", (2, 4), Some("PointMaker")).unwrap();
        assert_eq!(plan.error_name, "PointMakerError");
        assert!(
            plan.code.contains(
                "\n    pub(super) struct PointMaker {\n        x: ::core::option::Option<i32>,"
            ),
            "{}",
            plan.code
        );
        assert!(
            plan.file_text
                .ends_with("    }\n\n    impl ::core::error::Error for PointMakerError {}\n}\n"),
            "{}",
            plan.file_text
        );
        assert!(plan.file_text.contains("    }\n\n    /// Builds a `Point`"));
    }

    #[test]
    fn unsupported_shapes_are_refused_with_the_reason() {
        for (text, name, expected) in [
            ("pub struct P(u8, u16);\n", "P", "tuple struct"),
            ("pub struct P;\n", "P", "unit struct"),
            ("pub enum P {\n    A,\n}\n", "P", "is an enum"),
            ("pub union P {\n    a: u8,\n}\n", "P", "is a union"),
            (
                "#[cfg(test)]\npub struct P {\n    a: u8,\n}\n",
                "P",
                "`#[cfg(…)]`",
            ),
            (
                "pub struct P {\n    #[cfg(feature = \"x\")]\n    a: u8,\n}\n",
                "P",
                "field `a` of `P` carries `#[cfg(…)]`",
            ),
            (
                "pub struct P {\n    #[cfg_attr(test, allow(unused))]\n    a: u8,\n}\n",
                "P",
                "`#[cfg_attr(…)]`",
            ),
            (
                "#[my_macro]\npub struct P {\n    a: u8,\n}\n",
                "P",
                "attribute macro",
            ),
            (
                "pub struct P {\n    next: Option<Box<Self>>,\n}\n",
                "P",
                "spells `Self`",
            ),
            ("pub struct P {\n    a: u8 = 3,\n}\n", "P", "default value"),
            (
                "pub struct P {\n    build: u8,\n}\n",
                "P",
                "setter called `build`",
            ),
            (
                "pub struct P {\n    new: u8,\n}\n",
                "P",
                "setter called `new`",
            ),
            (
                "pub struct P {\n    a: u8,\n} fn f() {}\n",
                "P",
                "shares its last line",
            ),
        ] {
            let err = plan(text, name, (1, 4), None).expect_err(text);
            let err = format!("{err:#}");
            assert!(err.contains(expected), "{text}\n=> {err}");
        }
    }

    #[test]
    fn names_already_in_the_file_are_refused() {
        for text in [
            "pub struct P {\n    a: u8,\n}\npub struct PBuilder;\n",
            "use other::PBuilderError;\npub struct P {\n    a: u8,\n}\n",
            "pub struct P {\n    a: u8,\n}\nfn f() { let PBuilder = 1; }\n",
        ] {
            let err = format!("{:#}", plan(text, "P", (1, 4), None).expect_err(text));
            assert!(err.contains("already appears in this file"), "{err}");
        }
        // In a comment or a string it is not a name.
        let text =
            "// PBuilder\npub struct P {\n    a: &'static str,\n}\nconst S: &str = \"PBuilder\";\n";
        assert!(plan(text, "P", (1, 4), None).is_ok());
        let err = format!(
            "{:#}",
            plan("pub struct P {\n    a: u8,\n}\n", "P", (1, 3), Some("P")).unwrap_err()
        );
        assert!(err.contains("struct's own name"), "{err}");
        let err = format!(
            "{:#}",
            plan("pub struct P {\n    a: u8,\n}\n", "P", (1, 3), Some("fn")).unwrap_err()
        );
        assert!(err.contains("not an identifier"), "{err}");
    }

    #[test]
    fn helper_attributes_next_to_a_derive_are_noted_not_refused() {
        let text = "#[derive(Serialize)]\n#[serde(rename_all = \"camelCase\")]\n#[rustfmt::skip]\npub struct P {\n    #[serde(default)]\n    a: u8,\n}\n";
        let plan = plan(text, "P", (1, 7), None).unwrap();
        assert_eq!(plan.declaration_lines, (1, 7));
        assert_eq!(plan.notes.len(), 1, "{:?}", plan.notes);
        assert!(plan.notes[0].contains("`#[serde]`"), "{:?}", plan.notes);
    }

    #[test]
    fn many_fields_are_all_generated() {
        let mut text = String::from("pub struct Wide {\n");
        for i in 0..64 {
            text.push_str(&format!("    pub f{i}: u{},\n", 8 << (i % 4)));
        }
        text.push_str("}\n");
        let plan = plan(&text, "Wide", (1, 66), None).unwrap();
        assert_eq!(plan.fields.len(), 64);
        assert!(
            plan.code
                .contains("pub fn f63(mut self, value: u64) -> Self {")
        );
        assert!(
            plan.code
                .contains("f63: self.f63.ok_or(WideBuilderError { field: \"f63\" })?,")
        );
    }
}
