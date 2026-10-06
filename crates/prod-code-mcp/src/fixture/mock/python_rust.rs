/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::MethodSignature;

/// Python mock generator: mock class with calls recording and stubs dict.
pub(crate) fn generate_python_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!(
        "class {mock_name}:\n    \"\"\"Mock implementation of {type_name} for testing.\"\"\"\n\n    def __init__(self):\n        self.calls: list[tuple[str, tuple, dict]] = []\n        self._stubs: dict[str, any] = {{}}\n"
    );

    for (name, ty) in fields {
        let val = py_default_for_type(ty);
        out.push_str(&format!("        self.{name}: {ty} = {val}\n"));
    }
    if methods.is_empty() && fields.is_empty() {
        out.push_str("        pass\n");
    }
    out.push('\n');

    for m in methods {
        let mut params_list = vec!["self".to_string()];
        let mut param_names = Vec::new();
        for (n, t) in &m.params {
            let param_str = if t.is_empty() {
                n.clone()
            } else {
                format!("{n}: {t}")
            };
            params_list.push(param_str);
            param_names.push(n.clone());
        }
        let params_sig = params_list.join(", ");
        let ret_annotation = m
            .return_type
            .as_deref()
            .map(|r| format!(" -> {r}"))
            .unwrap_or_default();

        let tuple_expr = match param_names.len() {
            0 => "()".to_string(),
            1 => format!("({},)", param_names[0]),
            _ => format!("({})", param_names.join(", ")),
        };
        let call_args = param_names.join(", ");

        out.push_str(&format!(
            "    def {}({}){}:\n",
            m.name, params_sig, ret_annotation
        ));
        out.push_str(&format!(
            "        self.calls.append((\"{}\", {tuple_expr}, {{}}))\n",
            m.name
        ));
        out.push_str(&format!(
            "        if \"{}\" in self._stubs:\n            handler = self._stubs[\"{}\"]\n            return handler({call_args}) if callable(handler) else handler\n",
            m.name, m.name
        ));

        let ret = m.return_type.as_deref().unwrap_or("None");
        let default_val = py_default_for_type(ret);
        out.push_str(&format!("        return {default_val}\n\n"));
    }

    out.trim_end().to_string()
}

fn py_default_for_type(ty: &str) -> String {
    let t = ty.trim();
    if t.starts_with("list") || t.starts_with("List") {
        return "[]".to_string();
    }
    if t.starts_with("dict") || t.starts_with("Dict") {
        return "{}".to_string();
    }
    if t.starts_with("set") || t.starts_with("Set") {
        return "set()".to_string();
    }
    if t.starts_with("Optional") {
        return "None".to_string();
    }
    match t {
        "str" => "\"\"".to_string(),
        "int" => "0".to_string(),
        "float" => "0.0".to_string(),
        "bool" => "False".to_string(),
        "None" => "None".to_string(),
        "datetime" | "datetime.datetime" => "datetime.datetime(2026, 1, 1)".to_string(),
        _ => "None".to_string(),
    }
}

/// Rust mock generator: mock struct with thread-safe Mutex call recording and trait impl.
pub(crate) fn generate_rust_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!(
        "pub struct {mock_name} {{\n    pub calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,\n"
    );
    for (name, ty) in fields {
        out.push_str(&format!("    pub {name}: {ty},\n"));
    }
    out.push_str("}\n\n");

    out.push_str(&format!(
        "impl {mock_name} {{\n    pub fn new() -> Self {{\n        Self {{\n            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),\n"
    ));
    for (name, ty) in fields {
        let default_val = rust_default_for_type(ty);
        out.push_str(&format!("            {name}: {default_val},\n"));
    }
    out.push_str("        }\n    }\n}\n\n");

    out.push_str(&format!(
        "impl Default for {mock_name} {{\n    fn default() -> Self {{\n        Self::new()\n    }}\n}}\n\n"
    ));

    if !methods.is_empty() {
        out.push_str(&format!("impl {type_name} for {mock_name} {{\n"));
        for m in methods {
            let mut params_list = Vec::new();
            for (n, t) in &m.params {
                if n == "&self" || n == "&mut self" || n == "self" {
                    params_list.push(n.clone());
                } else if !n.is_empty() && !t.is_empty() {
                    params_list.push(format!("{n}: {t}"));
                } else if !t.is_empty() {
                    params_list.push(t.clone());
                }
            }
            if !params_list
                .iter()
                .any(|p| p.starts_with('&') || p == "self")
            {
                params_list.insert(0, "&self".to_string());
            }
            let params_sig = params_list.join(", ");
            let ret_sig = match &m.return_type {
                Some(ret) if ret != "()" => format!(" -> {ret}"),
                _ => String::new(),
            };

            out.push_str(&format!(
                "    fn {}({}){} {{\n",
                m.name, params_sig, ret_sig
            ));
            out.push_str(&format!(
                "        self.calls.lock().unwrap().push(\"{}\".to_string());\n",
                m.name
            ));
            if let Some(ret) = &m.return_type
                && ret != "()"
            {
                let default_ret = rust_default_for_type(ret);
                out.push_str(&format!("        {default_ret}\n"));
            }
            out.push_str("    }\n\n");
        }
        out.push_str("}\n");
    }

    out.trim_end().to_string()
}

fn rust_default_for_type(ty: &str) -> String {
    let t = ty.trim();
    if t == "()" {
        return "()".to_string();
    }
    if t == "bool" {
        return "false".to_string();
    }
    if matches!(
        t,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
    ) {
        return "0".to_string();
    }
    if t == "f32" || t == "f64" {
        return "0.0".to_string();
    }
    if t == "String" {
        return "String::new()".to_string();
    }
    if t == "&str" || t == "str" {
        return "\"\"".to_string();
    }
    if t.starts_with("Option<") {
        return "None".to_string();
    }
    if t.starts_with("Vec<") {
        return "Vec::new()".to_string();
    }
    if t.starts_with("HashMap<") {
        return "std::collections::HashMap::new()".to_string();
    }
    if t.starts_with("Result<") {
        return "Ok(Default::default())".to_string();
    }
    "Default::default()".to_string()
}
