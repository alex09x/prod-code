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
use super::helpers::inner_bracket_type;
use crate::fixture::polyglot::split_comma_top_level;

/// Go mock generator: idiomatic struct with func fields and method delegations.
pub(crate) fn generate_go_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!(
        "// {mock_name} is a mock implementation of {type_name} for testing.\ntype {mock_name} struct {{\n"
    );

    for (name, ty) in fields {
        out.push_str(&format!("    {name} {ty}\n"));
    }
    for m in methods {
        let params_sig = m
            .params
            .iter()
            .map(|(n, t)| {
                if n.is_empty() {
                    t.clone()
                } else {
                    format!("{n} {t}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let ret_sig = match &m.return_type {
            Some(ret) if ret.contains(',') || ret.contains(' ') => format!("({ret})"),
            Some(ret) => ret.clone(),
            None => String::new(),
        };
        let ret_space = if ret_sig.is_empty() {
            String::new()
        } else {
            format!(" {ret_sig}")
        };
        out.push_str(&format!(
            "    {}Func func({}){}\n",
            m.name, params_sig, ret_space
        ));
    }
    out.push_str("    Calls []string\n}\n\n");

    for m in methods {
        let params_decl = m
            .params
            .iter()
            .enumerate()
            .map(|(i, (n, t))| {
                let name = if n.is_empty() {
                    format!("arg{i}")
                } else {
                    n.clone()
                };
                format!("{name} {t}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let param_names = m
            .params
            .iter()
            .enumerate()
            .map(|(i, (n, _))| {
                if n.is_empty() {
                    format!("arg{i}")
                } else {
                    n.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let ret_sig = match &m.return_type {
            Some(ret) if ret.contains(',') || ret.contains(' ') => format!(" ({ret})"),
            Some(ret) => format!(" {ret}"),
            None => String::new(),
        };

        out.push_str(&format!(
            "func (m *{mock_name}) {}({}){} {{\n",
            m.name, params_decl, ret_sig
        ));
        out.push_str(&format!("    m.Calls = append(m.Calls, \"{}\")\n", m.name));
        out.push_str(&format!("    if m.{}Func != nil {{\n", m.name));
        if m.return_type.is_some() {
            out.push_str(&format!(
                "        return m.{}Func({})\n",
                m.name, param_names
            ));
        } else {
            out.push_str(&format!(
                "        m.{}Func({})\n        return\n",
                m.name, param_names
            ));
        }
        out.push_str("    }\n");

        if let Some(ret) = &m.return_type {
            let default_ret = go_default_returns(ret);
            out.push_str(&format!("    return {default_ret}\n"));
        }
        out.push_str("}\n\n");
    }

    out.trim_end().to_string()
}

fn go_default_returns(ret: &str) -> String {
    let clean = ret.trim().trim_start_matches('(').trim_end_matches(')');
    let parts = split_comma_top_level(clean);
    parts
        .iter()
        .map(|p| match p.trim() {
            "error" => "nil".to_string(),
            "bool" => "false".to_string(),
            "string" => "\"\"".to_string(),
            "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
            | "uint32" | "uint64" | "byte" | "rune" | "uintptr" => "0".to_string(),
            "float32" | "float64" => "0.0".to_string(),
            p if p.starts_with('*')
                || p.starts_with("[]")
                || p.starts_with("map[")
                || p.starts_with("chan")
                || p.starts_with("<-chan")
                || p.starts_with("func")
                || p.starts_with("interface {") =>
            {
                "nil".to_string()
            }
            p => format!("*new({p})"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// TypeScript mock generator: mock class implementing interface and createMock factory.
pub(crate) fn generate_ts_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!(
        "export class {mock_name} implements {type_name} {{\n    public calls: Array<{{ method: string; args: any[] }}> = [];\n"
    );

    for (name, ty) in fields {
        let val = ts_default_for_type(ty);
        out.push_str(&format!("    public {name}: {ty} = {val};\n"));
    }
    for m in methods {
        let params_sig = m
            .params
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = m.return_type.as_deref().unwrap_or("void");
        out.push_str(&format!(
            "    public {}Handler?: ({}) => {};\n",
            m.name, params_sig, ret
        ));
    }
    out.push('\n');

    for m in methods {
        let params_sig = m
            .params
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let param_names = m
            .params
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let ret = m.return_type.as_deref().unwrap_or("void");
        let is_async = ret.starts_with("Promise<");

        let async_prefix = if is_async { "async " } else { "" };
        out.push_str(&format!(
            "    public {async_prefix}{}({}): {} {{\n",
            m.name, params_sig, ret
        ));
        out.push_str(&format!(
            "        this.calls.push({{ method: \"{}\", args: [{}] }});\n",
            m.name, param_names
        ));
        out.push_str(&format!("        if (this.{}Handler) {{\n", m.name));
        if ret == "void" {
            out.push_str(&format!(
                "            this.{}Handler({});\n            return;\n",
                m.name, param_names
            ));
        } else if is_async {
            out.push_str(&format!(
                "            return await this.{}Handler({});\n",
                m.name, param_names
            ));
        } else {
            out.push_str(&format!(
                "            return this.{}Handler({});\n",
                m.name, param_names
            ));
        }
        out.push_str("        }\n");

        let default_ret = ts_default_for_type(ret);
        if ret == "void" {
            // no return statement needed
        } else if is_async {
            let inner = inner_bracket_type(ret, "Promise").unwrap_or("void");
            if inner == "void" {
                out.push_str("        return;\n");
            } else {
                let inner_default = ts_default_for_type(inner);
                out.push_str(&format!("        return {inner_default};\n"));
            }
        } else {
            out.push_str(&format!("        return {default_ret};\n"));
        }
        out.push_str("    }\n\n");
    }
    out.push_str("}\n\n");

    // Factory function: createMock<TypeName>
    out.push_str(&format!(
        "export const create{mock_name} = (overrides?: Partial<{type_name}>): {type_name} => ({{\n"
    ));
    for (name, ty) in fields {
        let default_val = ts_default_for_type(ty);
        out.push_str(&format!("    {name}: {default_val},\n"));
    }
    for m in methods {
        let params_sig = m
            .params
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let ret = m.return_type.as_deref().unwrap_or("void");
        let default_ret = if ret == "void" {
            "{}".to_string()
        } else if ret.starts_with("Promise<") {
            let inner = inner_bracket_type(ret, "Promise").unwrap_or("void");
            if inner == "void" {
                "Promise.resolve()".to_string()
            } else {
                format!("Promise.resolve({})", ts_default_for_type(inner))
            }
        } else {
            ts_default_for_type(ret)
        };
        out.push_str(&format!(
            "    {}: ({}) => {default_ret},\n",
            m.name, params_sig
        ));
    }
    out.push_str("    ...overrides,\n});");

    out
}

fn ts_default_for_type(ty: &str) -> String {
    let t = ty.trim();
    if t.ends_with("[]") || t.starts_with("Array<") {
        return "[]".to_string();
    }
    match t {
        "string" => "\"\"".to_string(),
        "number" => "0".to_string(),
        "boolean" => "false".to_string(),
        "void" => "undefined".to_string(),
        "any" | "unknown" => "null".to_string(),
        "Date" => "new Date(0)".to_string(),
        t if t.starts_with("Record<") => "{}".to_string(),
        _ => format!("{{}} as {t}"),
    }
}
