//! Polyglot interface extraction across TypeScript/JavaScript, Go, Python, C++, Swift, and Rust (Roadmap 7.1).
//!
//! Extracts method signatures from classes/structs into an interface/protocol/abstract base class,
//! updates the type declaration to implement/inherit/conform to the interface, and pre-validates
//! edits with in-memory overlays and optional compiler verification. For Rust, dispatches directly
//! to `crate::extract_trait::extract_trait`.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Result of extracting an interface.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractInterfaceResult {
    pub type_name: String,
    pub interface_name: String,
    pub methods: Vec<String>,
    pub files_modified: Vec<String>,
    #[serde(skip)]
    pub overlays: Vec<(PathBuf, String)>,
    pub diff: String,
    pub applied: bool,
    pub verified: bool,
    pub diagnostics: Vec<String>,
}

impl ExtractInterfaceResult {
    pub fn render(&self, max_diff_len: usize) -> String {
        let mut out = format!(
            "`interface {}` for `{}`\n- extracted methods: {}\n- applied: {}\n",
            self.interface_name,
            self.type_name,
            if self.methods.is_empty() {
                "none".to_string()
            } else {
                self.methods.join(", ")
            },
            self.applied,
        );
        if !self.diagnostics.is_empty() {
            out.push_str(&format!("- diagnostics ({}):\n", self.diagnostics.len()));
            for d in &self.diagnostics {
                out.push_str(&format!("  • {d}\n"));
            }
        }
        if !self.diff.is_empty() {
            out.push_str("\nDiff:\n```diff\n");
            if self.diff.len() > max_diff_len {
                out.push_str(&self.diff[..max_diff_len]);
                out.push_str("\n... [truncated]\n");
            } else {
                out.push_str(&self.diff);
            }
            out.push_str("```\n");
        }
        out
    }
}

// ============================================================================
// TypeScript / JavaScript
// ============================================================================

pub fn extract_interface_ts(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let class_pos = text
        .find(&class_marker)
        .with_context(|| format!("class `{symbol}` not found in TypeScript source"))?;

    let open_brace = text[class_pos..]
        .find('{')
        .map(|idx| class_pos + idx)
        .context("class body opening brace not found")?;

    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)
        .context("class body matching closing brace not found")?;

    let inner = &text[open_brace + 1..close_brace];
    let mut extracted_methods = Vec::new();
    let mut interface_sigs = Vec::new();
    let mut depth = 0;

    for line in inner.lines() {
        let trimmed = line.trim();
        // Skip constructors, private fields (#), and comments
        if trimmed.starts_with("constructor")
            || trimmed.starts_with("private ")
            || trimmed.starts_with('#')
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.is_empty()
        {
            for c in line.chars() {
                if c == '{' {
                    depth += 1;
                } else if c == '}' && depth > 0 {
                    depth -= 1;
                }
            }
            continue;
        }

        if depth == 0 {
            // Check if line contains a method signature: `methodName(...)`
            if let Some(open_paren) = trimmed.find('(') {
                let prefix = trimmed[..open_paren].trim();
                let method_name = prefix
                    .split_whitespace()
                    .last()
                    .unwrap_or(prefix)
                    .trim_start_matches("async ")
                    .trim_start_matches("public ");

                if !method_name.is_empty()
                    && method_name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && (target_methods.is_empty() || target_methods.iter().any(|m| m == method_name))
                {
                    // Find closing paren and return type
                    if let Some(close_paren) = trimmed.find(')') {
                        let rest_after_paren = trimmed[close_paren + 1..].trim();
                        let ret_type = if let Some(stripped) = rest_after_paren.strip_prefix(':') {
                            stripped.split('{').next().unwrap_or(stripped).trim()
                        } else {
                            "any"
                        };

                        let params = &trimmed[open_paren + 1..close_paren];
                        let sig = format!("    {method_name}({params}): {ret_type};");
                        interface_sigs.push(sig);
                        extracted_methods.push(method_name.to_string());
                    }
                }
            }
        }

        for c in line.chars() {
            if c == '{' {
                depth += 1;
            } else if c == '}' && depth > 0 {
                depth -= 1;
            }
        }
    }

    if extracted_methods.is_empty() {
        bail!("no matching methods found in class `{symbol}` to extract");
    }

    let interface_def = format!(
        "export interface {interface_name} {{\n{}\n}}\n\n",
        interface_sigs.join("\n")
    );

    // Update class declaration with `implements {interface_name}`
    let mut out = text.to_string();
    let header = &text[class_pos..open_brace];
    let new_header = if header.contains("implements ") {
        header.replace("implements ", &format!("implements {interface_name}, "))
    } else {
        format!("{header}implements {interface_name} ")
    };

    out.replace_range(class_pos..open_brace, &new_header);

    // Find class line start (including export if present)
    let class_line_start = text[..class_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(class_line_start, &interface_def);

    Ok((out, extracted_methods))
}

// ============================================================================
// Go
// ============================================================================

pub fn extract_interface_go(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let type_marker = format!("type {symbol} ");
    let type_pos = text
        .find(&type_marker)
        .with_context(|| format!("type `{symbol}` not found in Go source"))?;

    let mut extracted_methods = Vec::new();
    let mut interface_sigs = Vec::new();

    // Look for methods with receiver `(r *{symbol})` or `(r {symbol})`
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("func ") {
            continue;
        }

        // Must have receiver
        let Some(open_paren) = trimmed.find('(') else {
            continue;
        };
        let Some(close_paren) = trimmed[open_paren..].find(')') else {
            continue;
        };
        let recv_part = &trimmed[open_paren + 1..open_paren + close_paren];

        // Check if receiver references symbol
        let recv_words: Vec<&str> = recv_part.split_whitespace().collect();
        let is_target_recv = recv_words.iter().any(|w| {
            let clean = w.trim_start_matches('*');
            clean == symbol
        });

        if !is_target_recv {
            continue;
        }

        // After receiver, extract method name, params, and return types
        let after_recv = trimmed[open_paren + close_paren + 1..].trim();
        let Some(sig_open_paren) = after_recv.find('(') else {
            continue;
        };
        let method_name = after_recv[..sig_open_paren].trim();

        if method_name.is_empty() {
            continue;
        }

        if !target_methods.is_empty()
            && !target_methods.iter().any(|m| m == method_name)
        {
            continue;
        }

        let sig = after_recv.split('{').next().unwrap_or(after_recv).trim();
        interface_sigs.push(format!("\t{sig}"));
        extracted_methods.push(method_name.to_string());
    }

    if extracted_methods.is_empty() {
        bail!("no matching receiver methods found for type `{symbol}` to extract");
    }

    let interface_def = format!(
        "type {interface_name} interface {{\n{}\n}}\n\n",
        interface_sigs.join("\n")
    );

    let type_line_start = text[..type_pos].rfind('\n').map_or(0, |i| i + 1);
    let mut out = text.to_string();
    out.insert_str(type_line_start, &interface_def);

    Ok((out, extracted_methods))
}

// ============================================================================
// Python
// ============================================================================

pub fn extract_interface_python(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let class_pos = text
        .find(&class_marker)
        .with_context(|| format!("class `{symbol}` not found in Python source"))?;

    let colon_rel = text[class_pos..]
        .find(':')
        .context("class definition colon not found")?;
    let header_end = class_pos + colon_rel;

    let mut extracted_methods = Vec::new();
    let mut protocol_methods = Vec::new();

    // Iterate through lines following class definition
    let after_class = &text[header_end + 1..];
    let class_indent = text[..class_pos]
        .lines()
        .last()
        .map_or("", |l| &l[..l.chars().take_while(|c| *c == ' ' || *c == '\t').count()]);

    for line in after_class.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let line_indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        if line_indent <= class_indent.len() && !trimmed.starts_with('#') {
            // Reached next top-level or sibling declaration
            break;
        }

        if trimmed.starts_with("def ") {
            let Some(open_paren) = trimmed.find('(') else {
                continue;
            };
            let method_name = trimmed[4..open_paren].trim();

            // Skip dunder methods
            if method_name.starts_with("__") && method_name.ends_with("__") {
                continue;
            }

            if !target_methods.is_empty()
                && !target_methods.iter().any(|m| m == method_name)
            {
                continue;
            }

            let sig = trimmed.strip_suffix(':').unwrap_or(trimmed).trim();
            protocol_methods.push(format!("    {sig}:\n        ..."));
            extracted_methods.push(method_name.to_string());
        }
    }

    if extracted_methods.is_empty() {
        bail!("no matching methods found in class `{symbol}` to extract");
    }

    let protocol_def = format!(
        "class {interface_name}(Protocol):\n{}\n\n\n",
        protocol_methods.join("\n\n")
    );

    let mut out = text.to_string();

    // Update class header to inherit from Protocol
    let old_header = &text[class_pos..header_end];
    let new_header = if old_header.contains('(') {
        old_header.replace('(', &format!("({interface_name}, "))
    } else {
        format!("{old_header}({interface_name})")
    };
    out.replace_range(class_pos..header_end, &new_header);

    // Prepend protocol definition
    let class_line_start = text[..class_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(class_line_start, &protocol_def);

    // Ensure Protocol import is present
    if !out.contains("from typing import Protocol") && !out.contains("typing.Protocol") {
        out.insert_str(0, "from typing import Protocol\n\n");
    }

    Ok((out, extracted_methods))
}

// ============================================================================
// C++
// ============================================================================

pub fn extract_interface_cpp(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let struct_marker = format!("struct {symbol}");
    let class_pos = text
        .find(&class_marker)
        .or_else(|| text.find(&struct_marker))
        .with_context(|| format!("class/struct `{symbol}` not found in C++ source"))?;

    let open_brace = text[class_pos..]
        .find('{')
        .map(|idx| class_pos + idx)
        .context("class body opening brace not found")?;

    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)
        .context("class body matching closing brace not found")?;

    let inner = &text[open_brace + 1..close_brace];
    let mut extracted_methods = Vec::new();
    let mut interface_sigs = Vec::new();

    for line in inner.lines() {
        let trimmed = line.trim();
        // Skip destructor, private markers, comments
        if trimmed.starts_with('~')
            || trimmed.starts_with("private:")
            || trimmed.starts_with("protected:")
            || trimmed.starts_with("//")
            || trimmed.is_empty()
        {
            continue;
        }

        if let Some(open_paren) = trimmed.find('(') {
            let before_paren = trimmed[..open_paren].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if words.len() < 2 {
                continue; // Skip constructor or single identifier
            }

            let method_name = words.last().unwrap();
            if !target_methods.is_empty()
                && !target_methods.iter().any(|m| m == method_name)
            {
                continue;
            }

            let close_paren = trimmed.find(')').context("malformed parameter list")?;
            let params = &trimmed[open_paren + 1..close_paren];
            let ret_type = words[..words.len() - 1]
                .iter()
                .filter(|w| **w != "virtual" && **w != "inline" && **w != "explicit")
                .copied()
                .collect::<Vec<_>>()
                .join(" ");

            let after_paren = trimmed[close_paren + 1..].split('{').next().unwrap_or("").trim();
            let const_qual = if after_paren.contains("const") {
                " const"
            } else {
                ""
            };

            let sig = format!("    virtual {ret_type} {method_name}({params}){const_qual} = 0;");
            interface_sigs.push(sig);
            extracted_methods.push(method_name.to_string());
        }
    }

    if extracted_methods.is_empty() {
        bail!("no matching member functions found in `{symbol}` to extract");
    }

    let interface_def = format!(
        "class {interface_name} {{\npublic:\n    virtual ~{interface_name}() = default;\n{}\n}};\n\n",
        interface_sigs.join("\n")
    );

    let mut out = text.to_string();
    let header = &text[class_pos..open_brace];
    let header_trimmed = header.trim_end();
    let new_header = if header_trimmed.contains(':') {
        header_trimmed.replace(':', &format!(": public {interface_name}, "))
    } else {
        format!("{header_trimmed} : public {interface_name} ")
    };
    out.replace_range(class_pos..open_brace, &new_header);

    let class_line_start = text[..class_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(class_line_start, &interface_def);

    Ok((out, extracted_methods))
}

// ============================================================================
// Swift
// ============================================================================

pub fn extract_interface_swift(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let struct_marker = format!("struct {symbol}");
    let type_pos = text
        .find(&class_marker)
        .or_else(|| text.find(&struct_marker))
        .with_context(|| format!("class/struct `{symbol}` not found in Swift source"))?;

    let open_brace = text[type_pos..]
        .find('{')
        .map(|idx| type_pos + idx)
        .context("type body opening brace not found")?;

    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)
        .context("type body matching closing brace not found")?;

    let inner = &text[open_brace + 1..close_brace];
    let mut extracted_methods = Vec::new();
    let mut protocol_sigs = Vec::new();

    for line in inner.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("func ") && !trimmed.starts_with("mutating func ") {
            continue;
        }

        let after_func = if let Some(rest) = trimmed.strip_prefix("mutating func ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("func ") {
            rest
        } else {
            trimmed
        };

        let Some(open_paren) = after_func.find('(') else {
            continue;
        };
        let method_name = after_func[..open_paren].trim();

        if !target_methods.is_empty()
            && !target_methods.iter().any(|m| m == method_name)
        {
            continue;
        }

        let sig = trimmed.split('{').next().unwrap_or(trimmed).trim();
        protocol_sigs.push(format!("    {sig}"));
        extracted_methods.push(method_name.to_string());
    }

    if extracted_methods.is_empty() {
        bail!("no matching methods found in `{symbol}` to extract");
    }

    let protocol_def = format!(
        "protocol {interface_name} {{\n{}\n}}\n\n",
        protocol_sigs.join("\n")
    );

    let mut out = text.to_string();
    let header = &text[type_pos..open_brace];
    let header_trimmed = header.trim_end();
    let new_header = if header_trimmed.contains(':') {
        header_trimmed.replace(':', &format!(": {interface_name}, "))
    } else {
        format!("{header_trimmed}: {interface_name} ")
    };
    out.replace_range(type_pos..open_brace, &new_header);

    let type_line_start = text[..type_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(type_line_start, &protocol_def);

    Ok((out, extracted_methods))
}

// ============================================================================
// Orchestrator
// ============================================================================

#[allow(clippy::too_many_arguments)]
pub async fn extract_interface_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: &str,
    interface_name: &str,
    methods: &[String],
    line: u32,
    col: u32,
    migrate_callers: bool,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ExtractInterfaceResult> {
    let language = crate::lang::language_id_for_path(file).to_string();

    // If Rust, dispatch to existing extract_trait
    if language == "rust" {
        let res = crate::extract_trait::extract_trait_ext(
            remote,
            root,
            file,
            line,
            col,
            methods,
            interface_name,
            migrate_callers,
            apply,
            force,
        )
        .await?;

        let diff = res.render();
        let files_modified: Vec<String> = res.rewritten.iter().map(|(p, _)| p.clone()).collect();
        let overlays: Vec<(PathBuf, String)> = res
            .rewritten
            .into_iter()
            .map(|(p, t)| (PathBuf::from(p), t))
            .collect();

        return Ok(ExtractInterfaceResult {
            type_name: res.type_name,
            interface_name: res.trait_name,
            methods: res.methods,
            files_modified,
            overlays,
            diff,
            applied: res.applied,
            verified: true,
            diagnostics: res.diagnostics,
        });
    }

    let file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;

    let (mut transformed_text, extracted_methods) = match language.as_str() {
        "typescript" | "javascript" => {
            extract_interface_ts(&file_text, symbol, interface_name, methods)?
        }
        "go" => extract_interface_go(&file_text, symbol, interface_name, methods)?,
        "python" => extract_interface_python(&file_text, symbol, interface_name, methods)?,
        "cpp" | "c" => extract_interface_cpp(&file_text, symbol, interface_name, methods)?,
        "swift" => extract_interface_swift(&file_text, symbol, interface_name, methods)?,
        other => bail!("unsupported language for extract_interface: {other}"),
    };

    let mut overlays: Vec<(PathBuf, String)> = Vec::new();
    let mut files_modified: Vec<String> = Vec::new();

    if migrate_callers {
        let lang = crate::parameter_object::Language::of(file)
            .unwrap_or(crate::parameter_object::Language::TypeScript);
        let modified_files = crate::caller_migration::migrate_callers_in_workspace(
            root,
            file,
            &transformed_text,
            symbol,
            interface_name,
            &extracted_methods,
            lang,
        )?;
        for (p, t) in modified_files {
            if p == *file {
                transformed_text = t.clone();
            }
            files_modified.push(p.to_string_lossy().into_owned());
            overlays.push((p, t));
        }
    }

    if overlays.is_empty() {
        overlays.push((file.to_path_buf(), transformed_text.clone()));
        files_modified.push(file.to_string_lossy().into_owned());
    }

    let mut diff = String::new();
    for (p, new_t) in &overlays {
        let orig = if *p == *file {
            file_text.clone()
        } else {
            std::fs::read_to_string(p).unwrap_or_default()
        };
        let d = similar::TextDiff::from_lines(&orig, new_t)
            .unified_diff()
            .context_radius(2)
            .header(&p.to_string_lossy(), &p.to_string_lossy())
            .to_string();
        if !d.is_empty() {
            diff.push_str(&d);
        }
    }

    // Overlay validation
    let reports = crate::diagnostics::validate_texts(remote, root, &overlays, &[])
        .await
        .unwrap_or_default();
    let mut diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.source
                    .as_deref()
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default(),
                d.message,
                d.line,
                d.col
            )
        })
        .collect();

    let mut verified = false;
    if verify == Some("compile") {
        let files_to_compile: Vec<(String, String)> = overlays
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect();
        let check = crate::compile_check::check(remote, root, &files_to_compile).await?;
        verified = check.passed;
        if !check.passed {
            if !force {
                bail!("compiler verification failed:\n{}", check.errors.join("\n"));
            }
            diagnostics.push(format!("compiler errors: {}", check.errors.join("; ")));
        }
    }

    let has_fatal = !diagnostics.is_empty();
    if has_fatal && !force && apply {
        bail!(
            "refactoring rejected by validation:\n{}",
            diagnostics.join("\n")
        );
    }

    let applied = if apply {
        let mut file_map = BTreeMap::new();
        for (p, t) in &overlays {
            file_map.insert(p.clone(), t.clone());
        }
        let edit = crate::signature::whole_file_edit(&file_map);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        true
    } else {
        false
    };

    Ok(ExtractInterfaceResult {
        type_name: symbol.to_string(),
        interface_name: interface_name.to_string(),
        methods: extracted_methods,
        files_modified,
        overlays,
        diff,
        applied,
        verified,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_interface_ts() {
        let ts = r#"export class UserService {
    public async getUser(id: string): Promise<User> {
        return fetchUser(id);
    }

    public deleteUser(id: string): boolean {
        return true;
    }
}
"#;
        let (res, methods) = extract_interface_ts(ts, "UserService", "IUserService", &[]).unwrap();
        assert_eq!(methods, vec!["getUser", "deleteUser"]);
        assert!(res.contains("export interface IUserService {"));
        assert!(res.contains("getUser(id: string): Promise<User>;"));
        assert!(res.contains("deleteUser(id: string): boolean;"));
        assert!(res.contains("export class UserService implements IUserService {"));
    }

    #[test]
    fn test_extract_interface_go() {
        let go = r#"package user

type UserService struct {
    db DB
}

func (s *UserService) GetUser(id string) (*User, error) {
    return nil, nil
}

func (s *UserService) DeleteUser(id string) error {
    return nil
}
"#;
        let (res, methods) = extract_interface_go(go, "UserService", "UserReader", &["GetUser".to_string()]).unwrap();
        assert_eq!(methods, vec!["GetUser"]);
        assert!(res.contains("type UserReader interface {"));
        assert!(res.contains("GetUser(id string) (*User, error)"));
        let iface = &res[..res.find("type UserService").unwrap()];
        assert!(!iface.contains("DeleteUser"));
        assert!(res.contains("type UserService struct {"));
    }

    #[test]
    fn test_extract_interface_python() {
        let py = r#"class AccountService:
    def deposit(self, amount: float) -> bool:
        return True

    def withdraw(self, amount: float) -> bool:
        return True
"#;
        let (res, methods) = extract_interface_python(py, "AccountService", "AccountProtocol", &[]).unwrap();
        assert_eq!(methods, vec!["deposit", "withdraw"]);
        assert!(res.contains("from typing import Protocol"));
        assert!(res.contains("class AccountProtocol(Protocol):"));
        assert!(res.contains("def deposit(self, amount: float) -> bool:\n        ..."));
        assert!(res.contains("class AccountService(AccountProtocol):"));
    }

    #[test]
    fn test_extract_interface_cpp() {
        let cpp = r#"class Shape {
public:
    double area() const {
        return 0.0;
    }

    double perimeter() const {
        return 0.0;
    }
};
"#;
        let (res, methods) = extract_interface_cpp(cpp, "Shape", "IShape", &[]).unwrap();
        assert_eq!(methods, vec!["area", "perimeter"]);
        assert!(res.contains("class IShape {"));
        assert!(res.contains("virtual double area() const = 0;"));
        assert!(res.contains("class Shape : public IShape {"));
    }

    #[test]
    fn test_extract_interface_swift() {
        let swift = r#"struct PaymentService {
    func pay(amount: Double) -> Bool {
        return true
    }

    func refund(transactionId: String) -> Bool {
        return true
    }
}
"#;
        let (res, methods) = extract_interface_swift(swift, "PaymentService", "PaymentProtocol", &[]).unwrap();
        assert_eq!(methods, vec!["pay", "refund"]);
        assert!(res.contains("protocol PaymentProtocol {"));
        assert!(res.contains("func pay(amount: Double) -> Bool"));
        assert!(res.contains("struct PaymentService: PaymentProtocol {"));
    }
}
