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

/// C++ mock generator: class inheriting interface with call tracking.
pub(crate) fn generate_cpp_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!(
        "class {mock_name} : public {type_name} {{\npublic:\n    std::vector<std::string> calls;\n\n"
    );

    for (name, ty) in fields {
        let val = cpp_default_for_type(ty);
        out.push_str(&format!("    {ty} {name}{{{val}}};\n"));
    }
    for m in methods {
        let params_sig = m
            .params
            .iter()
            .map(|(n, t)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = m.return_type.as_deref().unwrap_or("void");

        out.push_str(&format!(
            "    {ret} {}({}) override {{\n",
            m.name, params_sig
        ));
        out.push_str(&format!("        calls.push_back(\"{}\");\n", m.name));
        if ret != "void" {
            let default_val = cpp_default_for_type(ret);
            out.push_str(&format!("        return {default_val};\n"));
        }
        out.push_str("    }\n\n");
    }
    out.push_str("};");
    out
}

fn cpp_default_for_type(ty: &str) -> String {
    let t = ty.trim();
    match t {
        "bool" => "false".to_string(),
        "int" | "long" | "size_t" | "uint32_t" | "int64_t" => "0".to_string(),
        "float" | "double" => "0.0".to_string(),
        "std::string" | "string" => "\"\"".to_string(),
        t if t.starts_with("std::vector") => "{}".to_string(),
        t if t.starts_with("std::map") => "{}".to_string(),
        _ => "{}".to_string(),
    }
}

/// Swift mock generator: class conforming to protocol with call tracking.
pub(crate) fn generate_swift_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out =
        format!("final class {mock_name}: {type_name} {{\n    var calls: [String] = []\n\n");

    for (name, ty) in fields {
        let val = swift_default_for_type(ty);
        out.push_str(&format!("    var {name}: {ty} = {val}\n"));
    }
    for m in methods {
        let params_sig = m
            .params
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let ret_sig = m
            .return_type
            .as_deref()
            .map(|r| format!(" -> {r}"))
            .unwrap_or_default();

        out.push_str(&format!(
            "    func {}({}){} {{\n",
            m.name, params_sig, ret_sig
        ));
        out.push_str(&format!("        calls.append(\"{}\")\n", m.name));
        if let Some(ret) = &m.return_type
            && ret != "Void"
            && ret != "()"
        {
            let default_val = swift_default_for_type(ret);
            out.push_str(&format!("        return {default_val}\n"));
        }
        out.push_str("    }\n\n");
    }
    out.push('}');
    out
}

fn swift_default_for_type(ty: &str) -> String {
    let t = ty.trim();
    if t.starts_with('[') && t.ends_with(']') {
        return "[]".to_string();
    }
    match t {
        "String" => "\"\"".to_string(),
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "UInt" | "UInt8" | "UInt16" | "UInt32"
        | "UInt64" => "0".to_string(),
        "Double" | "Float" => "0.0".to_string(),
        "Bool" => "false".to_string(),
        t if t.ends_with('?') => "nil".to_string(),
        _ => "nil".to_string(),
    }
}

/// Java mock generator: public class implementing interface with call tracking.
pub(crate) fn generate_java_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!(
        "public class {mock_name} implements {type_name} {{\n    public java.util.List<String> calls = new java.util.ArrayList<>();\n\n"
    );

    for (name, ty) in fields {
        let val = java_default_for_type(ty);
        out.push_str(&format!("    public {ty} {name} = {val};\n"));
    }
    for m in methods {
        let params_sig = m
            .params
            .iter()
            .map(|(n, t)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = m.return_type.as_deref().unwrap_or("void");
        out.push_str(&format!(
            "    @Override\n    public {ret} {}({}) {{\n",
            m.name, params_sig
        ));
        out.push_str(&format!("        calls.add(\"{}\");\n", m.name));
        if ret != "void" && ret != "Void" {
            let default_val = java_default_for_type(ret);
            out.push_str(&format!("        return {default_val};\n"));
        }
        out.push_str("    }\n\n");
    }
    out.push_str("}\n");
    out
}

fn java_default_for_type(ty: &str) -> String {
    let t = ty.trim();
    match t {
        "boolean" => "false".to_string(),
        "byte" | "short" | "int" | "long" => "0".to_string(),
        "float" => "0.0f".to_string(),
        "double" => "0.0".to_string(),
        "char" => "'\\0'".to_string(),
        "String" => "\"\"".to_string(),
        t if t.starts_with("List<") || t.starts_with("java.util.List<") => {
            "new java.util.ArrayList<>()".to_string()
        }
        t if t.starts_with("Map<") || t.starts_with("java.util.Map<") => {
            "new java.util.HashMap<>()".to_string()
        }
        t if t.starts_with("Set<") || t.starts_with("java.util.Set<") => {
            "new java.util.HashSet<>()".to_string()
        }
        _ => "null".to_string(),
    }
}
