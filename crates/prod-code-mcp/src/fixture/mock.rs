//! Test mock generator (Roadmap 8.5): compile-ready mock structs and classes
//! implementing interface/trait/protocol contracts with call tracking and configurable stubs.

use crate::parameter_object::Language;
use super::polyglot::split_comma_top_level;

/// One method signature in an interface, trait, or protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSignature {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub return_type: Option<String>,
}

/// Generates a test mock implementation of `type_name` for `language`.
pub fn generate_mock(
    language: Language,
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    match language {
        Language::Go => generate_go_mock(type_name, methods, fields),
        Language::TypeScript | Language::JavaScript => {
            generate_ts_mock(type_name, methods, fields)
        }
        Language::Python => generate_python_mock(type_name, methods, fields),
        Language::Rust => generate_rust_mock(type_name, methods, fields),
        Language::Cpp | Language::C => generate_cpp_mock(type_name, methods, fields),
        Language::Swift => generate_swift_mock(type_name, methods, fields),
        Language::Java => generate_java_mock(type_name, methods, fields),
    }
}

/// Go mock generator: idiomatic struct with func fields and method delegations.
fn generate_go_mock(
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
            out.push_str(&format!("        return m.{}Func({})\n", m.name, param_names));
        } else {
            out.push_str(&format!("        m.{}Func({})\n        return\n", m.name, param_names));
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
                || p.starts_with("interface {") => "nil".to_string(),
            p => format!("*new({p})"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// TypeScript mock generator: mock class implementing interface and createMock factory.
fn generate_ts_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!(
        "/** {mock_name} is a mock implementation of {type_name} for testing. */\nexport class {mock_name} implements {type_name} {{\n"
    );
    out.push_str("    public calls: { method: string; args: any[] }[] = [];\n\n");

    for (name, ty) in fields {
        let default_val = ts_default_for_type(ty);
        out.push_str(&format!("    public {name}: {ty} = {default_val};\n"));
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
        let async_kw = if is_async { "async " } else { "" };

        out.push_str(&format!(
            "    public {async_kw}{}({}): {} {{\n",
            m.name, params_sig, ret
        ));
        out.push_str(&format!(
            "        this.calls.push({{ method: \"{}\", args: [{}] }});\n",
            m.name, param_names
        ));
        out.push_str(&format!("        if (this.{}Handler) {{\n", m.name));
        if ret == "void" || ret == "Promise<void>" {
            out.push_str(&format!(
                "            {}this.{}Handler({});\n            return;\n",
                if is_async { "await " } else { "" },
                m.name,
                param_names
            ));
        } else {
            out.push_str(&format!(
                "            return {}this.{}Handler({});\n",
                if is_async { "await " } else { "" },
                m.name,
                param_names
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
        out.push_str(&format!("    {}: ({}) => {default_ret},\n", m.name, params_sig));
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

/// Python mock generator: mock class with calls recording and stubs dict.
fn generate_python_mock(
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
fn generate_rust_mock(
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
            if !params_list.iter().any(|p| p.starts_with('&') || p == "self") {
                params_list.insert(0, "&self".to_string());
            }
            let params_sig = params_list.join(", ");
            let ret_sig = match &m.return_type {
                Some(ret) if ret != "()" => format!(" -> {ret}"),
                _ => String::new(),
            };

            out.push_str(&format!("    fn {}({}){} {{\n", m.name, params_sig, ret_sig));
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
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
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

/// C++ mock generator: class inheriting interface with call tracking.
fn generate_cpp_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!("class {mock_name} : public {type_name} {{\npublic:\n    std::vector<std::string> calls;\n\n");

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
fn generate_swift_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!("final class {mock_name}: {type_name} {{\n    var calls: [String] = []\n\n");

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

        out.push_str(&format!("    func {}({}){} {{\n", m.name, params_sig, ret_sig));
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
    if t.ends_with('?') {
        return "nil".to_string();
    }
    if t.starts_with('[') && t.ends_with(']') {
        if t.contains(':') {
            return "[:]".to_string();
        }
        return "[]".to_string();
    }
    match t {
        "Bool" => "false".to_string(),
        "Int" | "UInt" | "Int64" | "Double" | "Float" => "0".to_string(),
        "String" => "\"\"".to_string(),
        "Date" => "Date(timeIntervalSince1970: 0)".to_string(),
        _ => "nil".to_string(),
    }
}

fn generate_java_mock(
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    let mock_name = format!("Mock{type_name}");
    let mut out = format!("public class {mock_name} implements {type_name} {{\n    public final java.util.List<String> calls = new java.util.ArrayList<>();\n\n");
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
        out.push_str(&format!("    @Override\n    public {ret} {}({}) {{\n", m.name, params_sig));
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
        t if t.starts_with("List<") || t.starts_with("java.util.List<") => "new java.util.ArrayList<>()".to_string(),
        t if t.starts_with("Map<") || t.starts_with("java.util.Map<") => "new java.util.HashMap<>()".to_string(),
        t if t.starts_with("Set<") || t.starts_with("java.util.Set<") => "new java.util.HashSet<>()".to_string(),
        _ => "null".to_string(),
    }
}

fn inner_bracket_type<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let t = ty.trim();
    if !t.starts_with(wrapper) {
        return None;
    }
    let open = t.find('<')?;
    let close = t.rfind('>')?;
    Some(t[open + 1..close].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_go_mock_for_interface() {
        let methods = vec![
            MethodSignature {
                name: "GetUser".to_string(),
                params: vec![
                    ("ctx".to_string(), "context.Context".to_string()),
                    ("id".to_string(), "string".to_string()),
                ],
                return_type: Some("*User, error".to_string()),
            },
            MethodSignature {
                name: "DeleteUser".to_string(),
                params: vec![("id".to_string(), "string".to_string())],
                return_type: Some("error".to_string()),
            },
        ];
        let mock = generate_mock(Language::Go, "UserService", &methods, &[]);
        assert!(mock.contains("type MockUserService struct"));
        assert!(mock.contains("GetUserFunc func(ctx context.Context, id string) (*User, error)"));
        assert!(mock.contains("DeleteUserFunc func(id string) error"));
        assert!(mock.contains("func (m *MockUserService) GetUser(ctx context.Context, id string) (*User, error)"));
        assert!(mock.contains("m.Calls = append(m.Calls, \"GetUser\")"));
        assert!(mock.contains("return nil, nil"));
    }

    #[test]
    fn generates_ts_mock_for_interface() {
        let methods = vec![MethodSignature {
            name: "fetch".to_string(),
            params: vec![("url".to_string(), "string".to_string())],
            return_type: Some("Promise<string>".to_string()),
        }];
        let mock = generate_mock(Language::TypeScript, "Client", &methods, &[]);
        assert!(mock.contains("export class MockClient implements Client"));
        assert!(mock.contains("fetchHandler?: (url: string) => Promise<string>"));
        assert!(mock.contains("async fetch(url: string): Promise<string>"));
        assert!(mock.contains("this.calls.push({ method: \"fetch\", args: [url] })"));
        assert!(mock.contains("export const createMockClient"));
    }

    #[test]
    fn generates_python_mock_for_protocol() {
        let methods = vec![MethodSignature {
            name: "process".to_string(),
            params: vec![("item".to_string(), "str".to_string())],
            return_type: Some("bool".to_string()),
        }];
        let mock = generate_mock(Language::Python, "Processor", &methods, &[]);
        assert!(mock.contains("class MockProcessor:"));
        assert!(mock.contains("def process(self, item: str) -> bool:"));
        assert!(mock.contains("self.calls.append((\"process\", (item,), {}))"));
        assert!(mock.contains("return False"));
    }

    #[test]
    fn generates_rust_mock_for_trait() {
        let methods = vec![MethodSignature {
            name: "execute".to_string(),
            params: vec![("&self".to_string(), String::new()), ("code".to_string(), "u32".to_string())],
            return_type: Some("bool".to_string()),
        }];
        let mock = generate_mock(Language::Rust, "Executor", &methods, &[]);
        assert!(mock.contains("pub struct MockExecutor"));
        assert!(mock.contains("impl Executor for MockExecutor"));
        assert!(mock.contains("fn execute(&self, code: u32) -> bool"));
        assert!(mock.contains("self.calls.lock().unwrap().push(\"execute\".to_string())"));
    }
}
