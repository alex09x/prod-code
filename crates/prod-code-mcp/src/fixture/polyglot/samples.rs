/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

fn inner_of_rust<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let t = ty.trim();
    let open = t.find('<')?;
    let close = t.rfind('>')?;
    let name = t[..open].rsplit("::").next()?.trim();
    if name == wrapper {
        Some(t[open + 1..close].trim())
    } else {
        None
    }
}

/// Generates a sample value for a type in the specified language, either default or randomized.
pub fn sample_value_for_type(
    language: Language,
    ty: &str,
    field_name: Option<&str>,
    randomized: bool,
) -> String {
    let t = ty.trim();
    if language == Language::Python && t.is_empty() {
        return "None".to_string();
    }
    let fn_opt = field_name.map(str::to_ascii_lowercase);
    let fn_ref = fn_opt.as_deref().unwrap_or("");

    match language {
        Language::Go => {
            if !randomized {
                match t {
                    "bool" => "false".to_string(),
                    "string" => "\"\"".to_string(),
                    "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
                    | "uint32" | "uint64" | "byte" | "rune" | "uintptr" => "0".to_string(),
                    "float32" | "float64" => "0.0".to_string(),
                    "time.Time" => "time.Time{}".to_string(),
                    "time.Duration" => "0".to_string(),
                    "error" => "nil".to_string(),
                    p if p.starts_with('*') || p.starts_with("[]") || p.starts_with("map[") => {
                        "nil".to_string()
                    }
                    other => format!("{other}{{}}"),
                }
            } else {
                match t {
                    "bool" => "true".to_string(),
                    "string" => {
                        if fn_ref.contains("id") {
                            "\"id_9823\"".to_string()
                        } else if fn_ref.contains("email") {
                            "\"user@example.com\"".to_string()
                        } else if fn_ref.contains("name") {
                            "\"test_sample_name\"".to_string()
                        } else if fn_ref.contains("url") || fn_ref.contains("uri") {
                            "\"https://example.com/api\"".to_string()
                        } else if fn_ref.contains("token") {
                            "\"tok_sec_7a8b9c\"".to_string()
                        } else {
                            format!("\"sample_{}\"", field_name.unwrap_or("val"))
                        }
                    }
                    "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
                    | "uint32" | "uint64" | "uintptr" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else if fn_ref.contains("age") {
                            "30".to_string()
                        } else if fn_ref.contains("count") || fn_ref.contains("total") {
                            "100".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "float32" | "float64" => {
                        if fn_ref.contains("price") || fn_ref.contains("amount") {
                            "99.95".to_string()
                        } else {
                            "3.14".to_string()
                        }
                    }
                    "time.Time" => "time.Date(2026, 1, 1, 12, 0, 0, 0, time.UTC)".to_string(),
                    "time.Duration" => "5 * time.Second".to_string(),
                    "[]string" => "[]string{\"sample_1\", \"sample_2\"}".to_string(),
                    "[]byte" => "[]byte(\"sample_bytes\")".to_string(),
                    "[]int" => "[]int{1, 2, 3}".to_string(),
                    p if p.starts_with("[]") => {
                        let inner = &p[2..];
                        format!(
                            "[]{inner}{{{}}}",
                            sample_value_for_type(language, inner, None, true)
                        )
                    }
                    p if p.starts_with('*') => {
                        let inner = &p[1..];
                        format!("&{inner}{{}}")
                    }
                    other => format!("{other}{{}}"),
                }
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if !randomized {
                match t {
                    "boolean" => "false".to_string(),
                    "number" => "0".to_string(),
                    "string" => "\"\"".to_string(),
                    "void" => "undefined".to_string(),
                    "any" | "unknown" => "null".to_string(),
                    "Date" => "new Date(0)".to_string(),
                    t if t.ends_with("[]") || t.starts_with("Array<") => "[]".to_string(),
                    t if t.starts_with("Record<") => "{}".to_string(),
                    other => format!("{{}} as {other}"),
                }
            } else {
                match t {
                    "boolean" => "true".to_string(),
                    "string" => {
                        if fn_ref.contains("id") {
                            "\"id_9823\"".to_string()
                        } else if fn_ref.contains("email") {
                            "\"user@example.com\"".to_string()
                        } else if fn_ref.contains("name") {
                            "\"test_sample_name\"".to_string()
                        } else if fn_ref.contains("url") {
                            "\"https://example.com/api\"".to_string()
                        } else {
                            format!("\"sample_{}\"", field_name.unwrap_or("val"))
                        }
                    }
                    "number" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else if fn_ref.contains("age") {
                            "30".to_string()
                        } else if fn_ref.contains("price") {
                            "99.95".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "Date" => "\"2026-01-01T00:00:00.000Z\"".to_string(),
                    "string[]" => "[\"sample_1\", \"sample_2\"]".to_string(),
                    "number[]" => "[10, 20, 30]".to_string(),
                    t if t.ends_with("[]") => "[]".to_string(),
                    t if t.starts_with("Record<") => "{\"key\": \"sample_val\"}".to_string(),
                    other => format!("{{}} as {other}"),
                }
            }
        }
        Language::Python => {
            if !randomized {
                match t {
                    "bool" => "False".to_string(),
                    "int" => "0".to_string(),
                    "float" => "0.0".to_string(),
                    "str" => "\"\"".to_string(),
                    "None" => "None".to_string(),
                    t if t.starts_with("list") || t.starts_with("List") => "[]".to_string(),
                    t if t.starts_with("dict") || t.starts_with("Dict") => "{}".to_string(),
                    t if t.starts_with("set") || t.starts_with("Set") => "set()".to_string(),
                    t if t.starts_with("Optional") => "None".to_string(),
                    other => format!("{other}()"),
                }
            } else {
                match t {
                    "bool" => "True".to_string(),
                    "str" => {
                        if fn_ref.contains("id") {
                            "\"id_9823\"".to_string()
                        } else if fn_ref.contains("email") {
                            "\"user@example.com\"".to_string()
                        } else if fn_ref.contains("name") {
                            "\"test_sample_name\"".to_string()
                        } else {
                            format!("\"sample_{}\"", field_name.unwrap_or("val"))
                        }
                    }
                    "int" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "float" => {
                        if fn_ref.contains("price") {
                            "99.95".to_string()
                        } else {
                            "3.14".to_string()
                        }
                    }
                    "list[str]" | "List[str]" => "[\"sample_1\", \"sample_2\"]".to_string(),
                    "list[int]" | "List[int]" => "[1, 2, 3]".to_string(),
                    t if t.starts_with("list") || t.starts_with("List") => "[]".to_string(),
                    t if t.starts_with("dict") || t.starts_with("Dict") => {
                        "{\"key\": \"sample_val\"}".to_string()
                    }
                    other => format!("{other}()"),
                }
            }
        }
        Language::Rust => {
            if !randomized {
                match super::super::values::known_value(t) {
                    Some(v) => v,
                    None => "Default::default()".to_string(),
                }
            } else {
                match t {
                    "bool" => "true".to_string(),
                    "String" => {
                        format!("\"sample_{}\".to_string()", field_name.unwrap_or("val"))
                    }
                    "&str" | "str" => {
                        format!("\"sample_{}\"", field_name.unwrap_or("val"))
                    }
                    "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32"
                    | "u64" | "u128" | "usize" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else if fn_ref.contains("timeout") {
                            "30".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "f32" | "f64" => "3.14".to_string(),
                    "Duration" => "std::time::Duration::from_secs(30)".to_string(),
                    "SocketAddr" => "\"127.0.0.1:8080\".parse().unwrap()".to_string(),
                    p if p.starts_with("Vec<") => {
                        let inner = inner_of_rust(p, "Vec").unwrap_or("String");
                        let sample_inner = sample_value_for_type(language, inner, None, true);
                        format!("vec![{sample_inner}]")
                    }
                    p if p.starts_with("Option<") => {
                        let inner = inner_of_rust(p, "Option").unwrap_or("String");
                        let sample_inner = sample_value_for_type(language, inner, None, true);
                        format!("Some({sample_inner})")
                    }
                    _ => match super::super::values::known_value(t) {
                        Some(v) => v,
                        None => "Default::default()".to_string(),
                    },
                }
            }
        }
        Language::Cpp | Language::C => {
            if !randomized {
                match t {
                    "bool" => "false".to_string(),
                    "int" | "long" | "size_t" | "uint32_t" | "int64_t" => "0".to_string(),
                    "float" | "double" => "0.0".to_string(),
                    "std::string" | "string" => "\"\"".to_string(),
                    _ => "{}".to_string(),
                }
            } else {
                match t {
                    "bool" => "true".to_string(),
                    "std::string" | "string" => {
                        format!("\"sample_{}\"", field_name.unwrap_or("val"))
                    }
                    "int" | "long" | "size_t" | "uint32_t" | "int64_t" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "float" | "double" => "3.14".to_string(),
                    _ => "{}".to_string(),
                }
            }
        }
        Language::Swift => {
            if !randomized {
                match t {
                    "Bool" => "false".to_string(),
                    "Int" | "UInt" | "Int64" => "0".to_string(),
                    "Double" | "Float" => "0.0".to_string(),
                    "String" => "\"\"".to_string(),
                    t if t.ends_with('?') => "nil".to_string(),
                    t if t.starts_with('[') && t.ends_with(']') => {
                        if t.contains(':') {
                            "[:]".to_string()
                        } else {
                            "[]".to_string()
                        }
                    }
                    other => format!("{other}()"),
                }
            } else {
                match t {
                    "Bool" => "true".to_string(),
                    "String" => {
                        format!("\"sample_{}\"", field_name.unwrap_or("val"))
                    }
                    "Int" | "UInt" | "Int64" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "Double" | "Float" => "3.14".to_string(),
                    _ => format!("{t}()"),
                }
            }
        }
        Language::Java => {
            if !randomized {
                match t {
                    "boolean" => "false".to_string(),
                    "String" => "\"\"".to_string(),
                    "byte" | "short" | "int" | "long" => "0".to_string(),
                    "float" => "0.0f".to_string(),
                    "double" => "0.0".to_string(),
                    "char" => "'\\0'".to_string(),
                    p if p.starts_with("List<") || p.starts_with("java.util.List<") => {
                        "java.util.Collections.emptyList()".to_string()
                    }
                    p if p.starts_with("Map<") || p.starts_with("java.util.Map<") => {
                        "java.util.Collections.emptyMap()".to_string()
                    }
                    p if p.starts_with("Set<") || p.starts_with("java.util.Set<") => {
                        "java.util.Collections.emptySet()".to_string()
                    }
                    _ => "null".to_string(),
                }
            } else {
                match t {
                    "boolean" => "true".to_string(),
                    "String" => format!("\"test_{}\"", fn_ref),
                    "int" | "long" => "42".to_string(),
                    "double" | "float" => "3.14".to_string(),
                    _ => "null".to_string(),
                }
            }
        }
    }
}
