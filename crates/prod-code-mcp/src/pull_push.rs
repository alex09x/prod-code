//! Pull up and push down refactorings across polyglot OOP and trait hierarchies (Roadmap 7.1.3).
//!
//! Supports class and trait hierarchies in:
//! - Python (`class Sub(Super):`)
//! - TypeScript / JavaScript (`class Sub extends Super`)
//! - C++ (`class Sub : public Super`)
//! - Swift (`class Sub: Super`)
//! - Rust (`trait Sub: Super`)
//!
//! Provides AST-aware member relocation, sibling deduplication, override modifier adjustment,
//! conflict detection, analyzer overlay verification, and transactional WorkspaceEdit application.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Outcome of a pull up or push down refactoring.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HierarchyRefactorResult {
    pub operation: String,
    pub source_class: String,
    pub target_classes: Vec<String>,
    pub members: Vec<String>,
    pub files_modified: Vec<String>,
    #[serde(skip)]
    pub overlays: Vec<(PathBuf, String)>,
    pub diff: String,
    pub applied: bool,
    pub verified: bool,
    pub diagnostics: Vec<String>,
}

impl HierarchyRefactorResult {
    pub fn render(&self, max_diff_len: usize) -> String {
        let op_name = if self.operation == "pull_up" {
            "Pull Up"
        } else {
            "Push Down"
        };
        let mut out = format!(
            "{op_name} — source: `{}` -> targets: {:?}\n",
            self.source_class, self.target_classes
        );
        out.push_str(&format!(
            "- members ({}): {}\n",
            self.members.len(),
            self.members.join(", ")
        ));
        out.push_str(&format!(
            "- files modified ({}): {}\n",
            self.files_modified.len(),
            self.files_modified.join(", ")
        ));
        out.push_str(&format!("- applied: {}\n", self.applied));
        out.push_str(&format!("- verified: {}\n", self.verified));
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

/// A parsed class, struct, or trait declaration.
#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub name: String,
    pub language: String,
    pub file_path: PathBuf,
    pub super_names: Vec<String>,
    pub decl_start: usize,
    pub decl_end: usize,
    pub body_start: usize,
    pub body_end: usize,
    pub indent: String,
    pub members: Vec<MemberDecl>,
}

/// A member (method, field, property, constant, associated item) inside a class or trait.
#[derive(Debug, Clone)]
pub struct MemberDecl {
    pub name: String,
    pub kind: MemberKind,
    pub is_override: bool,
    pub start_offset: usize,
    pub end_offset: usize,
    pub full_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Method,
    Field,
    Constant,
    AssociatedType,
}

/// Find matching closing brace `}` for `{` at `open_idx`, ignoring strings and comments.
pub fn find_matching_brace(text: &str, open_idx: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open_idx) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open_idx;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick = false;

    while i < bytes.len() {
        let b = bytes[i];
        let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };

        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if b == b'*' && next == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_single_quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_double_quote = false;
            }
            i += 1;
            continue;
        }
        if in_backtick {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'`' {
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        if b == b'/' && next == b'/' {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if b == b'/' && next == b'*' {
            in_block_comment = true;
            i += 2;
            continue;
        }

        if b == b'\'' {
            in_single_quote = true;
            i += 1;
            continue;
        }
        if b == b'"' {
            in_double_quote = true;
            i += 1;
            continue;
        }
        if b == b'`' {
            in_backtick = true;
            i += 1;
            continue;
        }

        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Parse classes and traits from source text based on language.
pub fn parse_classes_in_text(text: &str, language: &str, file_path: &Path) -> Vec<ClassDecl> {
    match language {
        "python" => parse_python_classes(text, file_path),
        "typescript" | "javascript" => parse_ts_classes(text, file_path, language),
        "cpp" | "c" => parse_cpp_classes(text, file_path),
        "swift" => parse_swift_classes(text, file_path),
        "rust" => parse_rust_traits(text, file_path),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Python Class Parser
// ---------------------------------------------------------------------------

fn parse_python_classes(text: &str, file_path: &Path) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut line_starts = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for l in &lines {
        line_starts.push(offset);
        offset += l.len() + 1; // +1 for \n
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("class ") && trimmed_start.contains(':') {
            let base_indent_len = line.len() - trimmed_start.len();
            let base_indent = &line[..base_indent_len];
            let after_class = trimmed_start["class ".len()..].trim_start();
            let colon_idx = match after_class.find(':') {
                Some(idx) => idx,
                None => {
                    i += 1;
                    continue;
                }
            };
            let class_header = after_class[..colon_idx].trim();
            let (name, super_names) = if let Some(paren_idx) = class_header.find('(') {
                let name = class_header[..paren_idx].trim().to_string();
                let bases_str = if class_header.ends_with(')') {
                    &class_header[paren_idx + 1..class_header.len() - 1]
                } else {
                    &class_header[paren_idx + 1..]
                };
                let bases: Vec<String> = bases_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (name, bases)
            } else {
                (class_header.to_string(), Vec::new())
            };

            let decl_start = line_starts[i];
            let body_start_line = i + 1;

            // Find body extent and member indentation
            let mut body_end_line = body_start_line;
            let mut member_indent = None;

            let mut j = body_start_line;
            while j < lines.len() {
                let body_line = lines[j];
                let trimmed = body_line.trim();
                if trimmed.is_empty() {
                    j += 1;
                    continue;
                }
                let current_indent_len = body_line.len() - body_line.trim_start().len();
                if current_indent_len <= base_indent_len {
                    break;
                }
                if member_indent.is_none() && !trimmed.starts_with('#') {
                    member_indent = Some(body_line[..current_indent_len].to_string());
                }
                body_end_line = j + 1;
                j += 1;
            }

            let body_indent = member_indent.unwrap_or_else(|| format!("{base_indent}    "));
            let decl_end = if body_end_line < lines.len() {
                line_starts[body_end_line]
            } else {
                text.len()
            };
            let body_start = if body_start_line < lines.len() {
                line_starts[body_start_line]
            } else {
                text.len()
            };
            let body_end = decl_end;

            // Parse members inside Python class
            let mut members = Vec::new();
            let mut k = body_start_line;
            while k < body_end_line {
                let cur = lines[k];
                let trimmed = cur.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    k += 1;
                    continue;
                }

                if cur.starts_with(&body_indent) {
                    let after_indent = &cur[body_indent.len()..];
                    // Check for decorators
                    let member_start_line = if after_indent.starts_with('@') {
                        let start_dec = k;
                        while k < body_end_line && lines[k].trim_start().starts_with('@') {
                            k += 1;
                        }
                        start_dec
                    } else {
                        k
                    };

                    if k >= body_end_line {
                        break;
                    }

                    let def_line = lines[k];
                    let def_trimmed = def_line.trim();
                    if let Some(after_def_raw) = def_trimmed.strip_prefix("def ") {
                        let after_def = after_def_raw.trim_start();
                        if let Some(paren_idx) = after_def.find('(') {
                            let member_name = after_def[..paren_idx].trim().to_string();
                            let member_start = line_starts[member_start_line];

                            // Method body extends until next line at body_indent
                            let mut method_end_line = k + 1;
                            while method_end_line < body_end_line {
                                let m_line = lines[method_end_line];
                                let m_trimmed = m_line.trim();
                                if m_trimmed.is_empty() || m_trimmed.starts_with('#') {
                                    method_end_line += 1;
                                    continue;
                                }
                                let m_indent_len = m_line.len() - m_line.trim_start().len();
                                if m_indent_len <= body_indent.len() {
                                    break;
                                }
                                method_end_line += 1;
                            }

                            // Trim trailing blank lines from member end
                            let mut last_non_blank = method_end_line;
                            while last_non_blank > member_start_line
                                && lines[last_non_blank - 1].trim().is_empty()
                            {
                                last_non_blank -= 1;
                            }

                            let member_end = if last_non_blank < lines.len() {
                                line_starts[last_non_blank - 1] + lines[last_non_blank - 1].len()
                            } else {
                                text.len()
                            };

                            let full_text = text[member_start..member_end].to_string();
                            members.push(MemberDecl {
                                name: member_name,
                                kind: MemberKind::Method,
                                is_override: false,
                                start_offset: member_start,
                                end_offset: member_end,
                                full_text,
                            });
                            k = method_end_line;
                            continue;
                        }
                    } else if after_indent.contains('=') || after_indent.contains(':') {
                        // Field / Constant
                        let first_ident = after_indent
                            .split(&[':', '=', ' '][..])
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !first_ident.is_empty()
                            && first_ident.chars().all(|c| c.is_alphanumeric() || c == '_')
                        {
                            let member_start = line_starts[k];
                            let member_end = line_starts[k] + lines[k].len();
                            let full_text = text[member_start..member_end].to_string();
                            members.push(MemberDecl {
                                name: first_ident.to_string(),
                                kind: MemberKind::Field,
                                is_override: false,
                                start_offset: member_start,
                                end_offset: member_end,
                                full_text,
                            });
                        }
                    }
                }
                k += 1;
            }

            classes.push(ClassDecl {
                name,
                language: "python".to_string(),
                file_path: file_path.to_path_buf(),
                super_names,
                decl_start,
                decl_end,
                body_start,
                body_end,
                indent: body_indent,
                members,
            });

            i = body_end_line;
            continue;
        }
        i += 1;
    }

    classes
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript Class Parser
// ---------------------------------------------------------------------------

fn parse_ts_classes(text: &str, file_path: &Path, language: &str) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        if let Some(pos) = text[idx..].find("class ") {
            let class_kw_pos = idx + pos;
            // Ensure "class" is a whole keyword
            if class_kw_pos > 0 && text.as_bytes()[class_kw_pos - 1].is_ascii_alphanumeric() {
                idx = class_kw_pos + 6;
                continue;
            }
            let after_kw = &text[class_kw_pos + 6..];
            let open_brace_rel = match after_kw.find('{') {
                Some(p) => p,
                None => {
                    idx = class_kw_pos + 6;
                    continue;
                }
            };
            let header = after_kw[..open_brace_rel].trim();
            let open_brace_pos = class_kw_pos + 6 + open_brace_rel;

            let close_brace_pos = match find_matching_brace(text, open_brace_pos) {
                Some(p) => p,
                None => {
                    idx = class_kw_pos + 6;
                    continue;
                }
            };

            // Parse class name and superclass from header: e.g. "Dog extends Animal"
            let tokens: Vec<&str> = header.split_whitespace().collect();
            if tokens.is_empty() {
                idx = close_brace_pos + 1;
                continue;
            }
            let class_name = tokens[0]
                .split('<')
                .next()
                .unwrap_or(tokens[0])
                .trim()
                .to_string();
            let mut super_names = Vec::new();
            if let Some(ext_pos) = tokens.iter().position(|&t| t == "extends")
                && let Some(base) = tokens.get(ext_pos + 1)
            {
                let base_clean = base.split('<').next().unwrap_or(base).trim();
                super_names.push(base_clean.to_string());
            }

            // Find decl_start (handle optional `export `, `abstract `)
            let mut line_start = class_kw_pos;
            while line_start > 0 && bytes[line_start - 1] != b'\n' {
                line_start -= 1;
            }
            let decl_start = line_start;
            let decl_end = close_brace_pos + 1;
            let body_start = open_brace_pos + 1;
            let body_end = close_brace_pos;

            // Default indent
            let indent = "    ".to_string();

            // Parse members inside body
            let body_text = &text[body_start..body_end];
            let members = parse_ts_members(body_text, body_start);

            classes.push(ClassDecl {
                name: class_name,
                language: language.to_string(),
                file_path: file_path.to_path_buf(),
                super_names,
                decl_start,
                decl_end,
                body_start,
                body_end,
                indent,
                members,
            });

            idx = close_brace_pos + 1;
        } else {
            break;
        }
    }

    classes
}

fn parse_ts_members(body: &str, body_offset: usize) -> Vec<MemberDecl> {
    let mut members = Vec::new();
    let lines: Vec<&str> = body.split('\n').collect();
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut cur = body_offset;
    for l in &lines {
        line_offsets.push(cur);
        cur += l.len() + 1;
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
            i += 1;
            continue;
        }

        let is_override = trimmed.contains("override ") || trimmed.starts_with("override\t");

        // Check for method: identifier followed by `(` and containing `{`
        if let Some(open_paren) = trimmed.find('(') {
            let before_paren = trimmed[..open_paren].trim();
            let ident = before_paren.split_whitespace().last().unwrap_or("");
            if !ident.is_empty()
                && ident.chars().all(|c| c.is_alphanumeric() || c == '_')
                && ident != "constructor"
                && ident != "if"
                && ident != "while"
                && ident != "for"
            {
                // Find matching brace for method body
                let member_start = line_offsets[i];
                if let Some(open_brace_in_body) = body[line_offsets[i] - body_offset..].find('{') {
                    let global_open = line_offsets[i] + open_brace_in_body;
                    if let Some(global_close) = find_matching_brace(body, global_open - body_offset)
                    {
                        let member_end = body_offset + global_close + 1;
                        let full_text =
                            body[member_start - body_offset..member_end - body_offset].to_string();
                        members.push(MemberDecl {
                            name: ident.to_string(),
                            kind: MemberKind::Method,
                            is_override,
                            start_offset: member_start,
                            end_offset: member_end,
                            full_text,
                        });
                        // Skip past method
                        while i < lines.len() && line_offsets[i] < member_end {
                            i += 1;
                        }
                        continue;
                    }
                }
            }
        }

        // Check for property / field: ends with `;`
        if let Some(before_semi_raw) = trimmed.strip_suffix(';') {
            let before_semi = before_semi_raw.trim();
            let before_assign = before_semi.split('=').next().unwrap_or(before_semi).trim();
            let before_colon = before_assign
                .split(':')
                .next()
                .unwrap_or(before_assign)
                .trim();
            let ident = before_colon.split_whitespace().last().unwrap_or("");
            if !ident.is_empty()
                && ident.chars().all(|c| c.is_alphanumeric() || c == '_')
                && ident != "return"
                && ident != "break"
            {
                let member_start = line_offsets[i];
                let member_end = line_offsets[i] + line.len();
                let full_text =
                    body[member_start - body_offset..member_end - body_offset].to_string();
                members.push(MemberDecl {
                    name: ident.to_string(),
                    kind: MemberKind::Field,
                    is_override,
                    start_offset: member_start,
                    end_offset: member_end,
                    full_text,
                });
            }
        }

        i += 1;
    }

    members
}

// ---------------------------------------------------------------------------
// C++ Class Parser
// ---------------------------------------------------------------------------

fn parse_cpp_classes(text: &str, file_path: &Path) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        let next_class = text[idx..].find("class ");
        let next_struct = text[idx..].find("struct ");
        let (kw_pos, kw_len) = match (next_class, next_struct) {
            (Some(c), Some(s)) if c < s => (idx + c, 6),
            (Some(_), Some(s)) => (idx + s, 7),
            (Some(c), None) => (idx + c, 6),
            (None, Some(s)) => (idx + s, 7),
            (None, None) => break,
        };

        if kw_pos > 0 && bytes[kw_pos - 1].is_ascii_alphanumeric() {
            idx = kw_pos + kw_len;
            continue;
        }

        let after_kw = &text[kw_pos + kw_len..];
        let open_brace_rel = match after_kw.find('{') {
            Some(p) => p,
            None => {
                idx = kw_pos + kw_len;
                continue;
            }
        };

        // Forward declarations: `class Foo;` before `{`
        let header = after_kw[..open_brace_rel].trim();
        if header.contains(';') {
            idx = kw_pos + kw_len;
            continue;
        }

        let open_brace_pos = kw_pos + kw_len + open_brace_rel;
        let close_brace_pos = match find_matching_brace(text, open_brace_pos) {
            Some(p) => p,
            None => {
                idx = kw_pos + kw_len;
                continue;
            }
        };

        let (class_name, super_names) = if let Some(colon_idx) = header.find(':') {
            let name = header[..colon_idx].trim().to_string();
            let bases_part = &header[colon_idx + 1..];
            let bases: Vec<String> = bases_part
                .split(',')
                .map(|b| {
                    let parts: Vec<&str> = b.split_whitespace().collect();
                    parts.last().cloned().unwrap_or("").to_string()
                })
                .filter(|s| !s.is_empty())
                .collect();
            (name, bases)
        } else {
            (
                header.split_whitespace().next().unwrap_or("").to_string(),
                Vec::new(),
            )
        };

        if class_name.is_empty() {
            idx = close_brace_pos + 1;
            continue;
        }

        let decl_start = kw_pos;
        let decl_end = if close_brace_pos + 1 < text.len() && bytes[close_brace_pos + 1] == b';' {
            close_brace_pos + 2
        } else {
            close_brace_pos + 1
        };
        let body_start = open_brace_pos + 1;
        let body_end = close_brace_pos;

        let body_text = &text[body_start..body_end];
        let members = parse_cpp_members(body_text, body_start);

        classes.push(ClassDecl {
            name: class_name,
            language: "cpp".to_string(),
            file_path: file_path.to_path_buf(),
            super_names,
            decl_start,
            decl_end,
            body_start,
            body_end,
            indent: "    ".to_string(),
            members,
        });

        idx = decl_end;
    }

    classes
}

fn parse_cpp_members(body: &str, body_offset: usize) -> Vec<MemberDecl> {
    let mut members = Vec::new();
    let lines: Vec<&str> = body.split('\n').collect();
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut cur = body_offset;
    for l in &lines {
        line_offsets.push(cur);
        cur += l.len() + 1;
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed == "public:"
            || trimmed == "protected:"
            || trimmed == "private:"
        {
            i += 1;
            continue;
        }

        let is_override = trimmed.contains("override") || trimmed.contains("final");

        // Method: contains `(` and either `{` or `;`
        if let Some(open_paren) = trimmed.find('(') {
            let before_paren = trimmed[..open_paren].trim();
            let ident = before_paren.split_whitespace().last().unwrap_or("");
            if !ident.is_empty() && ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
                let member_start = line_offsets[i];
                if let Some(open_brace_rel) = body[line_offsets[i] - body_offset..].find('{') {
                    let global_open = line_offsets[i] + open_brace_rel;
                    if let Some(global_close) = find_matching_brace(body, global_open - body_offset)
                    {
                        let member_end = body_offset + global_close + 1;
                        let full_text =
                            body[member_start - body_offset..member_end - body_offset].to_string();
                        members.push(MemberDecl {
                            name: ident.to_string(),
                            kind: MemberKind::Method,
                            is_override,
                            start_offset: member_start,
                            end_offset: member_end,
                            full_text,
                        });
                        while i < lines.len() && line_offsets[i] < member_end {
                            i += 1;
                        }
                        continue;
                    }
                } else if trimmed.ends_with(';') {
                    // Method declaration
                    let member_end = line_offsets[i] + line.len();
                    let full_text =
                        body[member_start - body_offset..member_end - body_offset].to_string();
                    members.push(MemberDecl {
                        name: ident.to_string(),
                        kind: MemberKind::Method,
                        is_override,
                        start_offset: member_start,
                        end_offset: member_end,
                        full_text,
                    });
                }
            }
        } else if let Some(before_semi_raw) = trimmed.strip_suffix(';') {
            // Field
            let before_semi = before_semi_raw.trim();
            let before_assign = before_semi.split('=').next().unwrap_or(before_semi).trim();
            let ident = before_assign.split_whitespace().last().unwrap_or("");
            if !ident.is_empty() && ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
                let member_start = line_offsets[i];
                let member_end = line_offsets[i] + line.len();
                let full_text =
                    body[member_start - body_offset..member_end - body_offset].to_string();
                members.push(MemberDecl {
                    name: ident.to_string(),
                    kind: MemberKind::Field,
                    is_override,
                    start_offset: member_start,
                    end_offset: member_end,
                    full_text,
                });
            }
        }

        i += 1;
    }

    members
}

// ---------------------------------------------------------------------------
// Swift Class Parser
// ---------------------------------------------------------------------------

fn parse_swift_classes(text: &str, file_path: &Path) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        if let Some(pos) = text[idx..].find("class ") {
            let class_kw_pos = idx + pos;
            if class_kw_pos > 0 && bytes[class_kw_pos - 1].is_ascii_alphanumeric() {
                idx = class_kw_pos + 6;
                continue;
            }
            let after_kw = &text[class_kw_pos + 6..];
            let open_brace_rel = match after_kw.find('{') {
                Some(p) => p,
                None => {
                    idx = class_kw_pos + 6;
                    continue;
                }
            };
            let header = after_kw[..open_brace_rel].trim();
            let open_brace_pos = class_kw_pos + 6 + open_brace_rel;

            let close_brace_pos = match find_matching_brace(text, open_brace_pos) {
                Some(p) => p,
                None => {
                    idx = class_kw_pos + 6;
                    continue;
                }
            };

            let (class_name, super_names) = if let Some(colon_idx) = header.find(':') {
                let name = header[..colon_idx].trim().to_string();
                let bases_part = &header[colon_idx + 1..];
                let bases: Vec<String> = bases_part
                    .split(',')
                    .map(|b| b.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (name, bases)
            } else {
                (
                    header.split_whitespace().next().unwrap_or("").to_string(),
                    Vec::new(),
                )
            };

            let decl_start = class_kw_pos;
            let decl_end = close_brace_pos + 1;
            let body_start = open_brace_pos + 1;
            let body_end = close_brace_pos;

            let body_text = &text[body_start..body_end];
            let members = parse_swift_members(body_text, body_start);

            classes.push(ClassDecl {
                name: class_name,
                language: "swift".to_string(),
                file_path: file_path.to_path_buf(),
                super_names,
                decl_start,
                decl_end,
                body_start,
                body_end,
                indent: "    ".to_string(),
                members,
            });

            idx = close_brace_pos + 1;
        } else {
            break;
        }
    }

    classes
}

fn parse_swift_members(body: &str, body_offset: usize) -> Vec<MemberDecl> {
    let mut members = Vec::new();
    let lines: Vec<&str> = body.split('\n').collect();
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut cur = body_offset;
    for l in &lines {
        line_offsets.push(cur);
        cur += l.len() + 1;
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            i += 1;
            continue;
        }

        let is_override = trimmed.contains("override ");

        // func
        if let Some(func_idx) = trimmed.find("func ") {
            let after_func = trimmed[func_idx + 5..].trim_start();
            if let Some(open_paren) = after_func.find('(') {
                let ident = after_func[..open_paren].trim();
                let member_start = line_offsets[i];
                if let Some(open_brace_rel) = body[line_offsets[i] - body_offset..].find('{') {
                    let global_open = line_offsets[i] + open_brace_rel;
                    if let Some(global_close) = find_matching_brace(body, global_open - body_offset)
                    {
                        let member_end = body_offset + global_close + 1;
                        let full_text =
                            body[member_start - body_offset..member_end - body_offset].to_string();
                        members.push(MemberDecl {
                            name: ident.to_string(),
                            kind: MemberKind::Method,
                            is_override,
                            start_offset: member_start,
                            end_offset: member_end,
                            full_text,
                        });
                        while i < lines.len() && line_offsets[i] < member_end {
                            i += 1;
                        }
                        continue;
                    }
                }
            }
        } else if trimmed.starts_with("var ")
            || trimmed.starts_with("let ")
            || trimmed.contains(" var ")
            || trimmed.contains(" let ")
        {
            // property
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some(kw_pos) = words.iter().position(|&w| w == "var" || w == "let")
                && let Some(ident_raw) = words.get(kw_pos + 1)
            {
                let ident = ident_raw.trim_end_matches(':').trim();
                let member_start = line_offsets[i];
                let member_end = line_offsets[i] + line.len();
                let full_text =
                    body[member_start - body_offset..member_end - body_offset].to_string();
                members.push(MemberDecl {
                    name: ident.to_string(),
                    kind: MemberKind::Field,
                    is_override,
                    start_offset: member_start,
                    end_offset: member_end,
                    full_text,
                });
            }
        }

        i += 1;
    }

    members
}

// ---------------------------------------------------------------------------
// Rust Trait Parser
// ---------------------------------------------------------------------------

fn parse_rust_traits(text: &str, file_path: &Path) -> Vec<ClassDecl> {
    let mut traits = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        if let Some(pos) = text[idx..].find("trait ") {
            let trait_kw_pos = idx + pos;
            if trait_kw_pos > 0 && bytes[trait_kw_pos - 1].is_ascii_alphanumeric() {
                idx = trait_kw_pos + 6;
                continue;
            }
            let after_kw = &text[trait_kw_pos + 6..];
            let open_brace_rel = match after_kw.find('{') {
                Some(p) => p,
                None => {
                    idx = trait_kw_pos + 6;
                    continue;
                }
            };
            let header = after_kw[..open_brace_rel].trim();
            let open_brace_pos = trait_kw_pos + 6 + open_brace_rel;

            let close_brace_pos = match find_matching_brace(text, open_brace_pos) {
                Some(p) => p,
                None => {
                    idx = trait_kw_pos + 6;
                    continue;
                }
            };

            let (trait_name, super_names) = if let Some(colon_idx) = header.find(':') {
                let name = header[..colon_idx]
                    .split('<')
                    .next()
                    .unwrap_or(&header[..colon_idx])
                    .trim()
                    .to_string();
                let bounds_part = &header[colon_idx + 1..];
                let bounds: Vec<String> = bounds_part
                    .split('+')
                    .map(|b| b.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (name, bounds)
            } else {
                let name = header
                    .split('<')
                    .next()
                    .unwrap_or(header)
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                (name, Vec::new())
            };

            let mut line_start = trait_kw_pos;
            while line_start > 0 && bytes[line_start - 1] != b'\n' {
                line_start -= 1;
            }
            let decl_start = line_start;
            let decl_end = close_brace_pos + 1;
            let body_start = open_brace_pos + 1;
            let body_end = close_brace_pos;

            let body_text = &text[body_start..body_end];
            let members = parse_rust_trait_members(body_text, body_start);

            traits.push(ClassDecl {
                name: trait_name,
                language: "rust".to_string(),
                file_path: file_path.to_path_buf(),
                super_names,
                decl_start,
                decl_end,
                body_start,
                body_end,
                indent: "    ".to_string(),
                members,
            });

            idx = close_brace_pos + 1;
        } else {
            break;
        }
    }

    traits
}

fn parse_rust_trait_members(body: &str, body_offset: usize) -> Vec<MemberDecl> {
    let mut members = Vec::new();
    let lines: Vec<&str> = body.split('\n').collect();
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut cur = body_offset;
    for l in &lines {
        line_offsets.push(cur);
        cur += l.len() + 1;
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            i += 1;
            continue;
        }

        // fn <name>
        if trimmed.starts_with("fn ") || trimmed.contains(" fn ") {
            let after_fn = if let Some(p) = trimmed.find("fn ") {
                &trimmed[p + 3..]
            } else {
                trimmed
            };
            let ident = after_fn
                .split(&['(', '<', ' '][..])
                .next()
                .unwrap_or("")
                .trim();
            if !ident.is_empty() {
                let member_start = line_offsets[i];
                if let Some(open_brace_rel) = body[line_offsets[i] - body_offset..].find('{') {
                    let global_open = line_offsets[i] + open_brace_rel;
                    if let Some(global_close) = find_matching_brace(body, global_open - body_offset)
                    {
                        let member_end = body_offset + global_close + 1;
                        let full_text =
                            body[member_start - body_offset..member_end - body_offset].to_string();
                        members.push(MemberDecl {
                            name: ident.to_string(),
                            kind: MemberKind::Method,
                            is_override: false,
                            start_offset: member_start,
                            end_offset: member_end,
                            full_text,
                        });
                        while i < lines.len() && line_offsets[i] < member_end {
                            i += 1;
                        }
                        continue;
                    }
                } else if trimmed.ends_with(';') {
                    let member_end = line_offsets[i] + line.len();
                    let full_text =
                        body[member_start - body_offset..member_end - body_offset].to_string();
                    members.push(MemberDecl {
                        name: ident.to_string(),
                        kind: MemberKind::Method,
                        is_override: false,
                        start_offset: member_start,
                        end_offset: member_end,
                        full_text,
                    });
                }
            }
        } else if let Some(after_type_raw) = trimmed.strip_prefix("type ") {
            let after_type = after_type_raw.trim_start();
            let ident = after_type
                .split(&[':', '=', ';', ' '][..])
                .next()
                .unwrap_or("")
                .trim();
            if !ident.is_empty() {
                let member_start = line_offsets[i];
                let member_end = line_offsets[i] + line.len();
                let full_text =
                    body[member_start - body_offset..member_end - body_offset].to_string();
                members.push(MemberDecl {
                    name: ident.to_string(),
                    kind: MemberKind::AssociatedType,
                    is_override: false,
                    start_offset: member_start,
                    end_offset: member_end,
                    full_text,
                });
            }
        } else if let Some(after_const_raw) = trimmed.strip_prefix("const ") {
            let after_const = after_const_raw.trim_start();
            let ident = after_const
                .split(&[':', '=', ';', ' '][..])
                .next()
                .unwrap_or("")
                .trim();
            if !ident.is_empty() {
                let member_start = line_offsets[i];
                let member_end = line_offsets[i] + line.len();
                let full_text =
                    body[member_start - body_offset..member_end - body_offset].to_string();
                members.push(MemberDecl {
                    name: ident.to_string(),
                    kind: MemberKind::Constant,
                    is_override: false,
                    start_offset: member_start,
                    end_offset: member_end,
                    full_text,
                });
            }
        }

        i += 1;
    }

    members
}

// ---------------------------------------------------------------------------
// Formatting & Workspace Discovery Helpers
// ---------------------------------------------------------------------------

/// Strip `override` or `final` modifiers from member text when pulling up to base.
pub fn strip_override_modifiers(text: &str, language: &str) -> String {
    match language {
        "typescript" | "javascript" | "swift" => {
            let lines: Vec<&str> = text.split('\n').collect();
            let mut out = Vec::with_capacity(lines.len());
            for line in lines {
                if let Some(pos) = line.find("override ") {
                    let cleaned = format!("{}{}", &line[..pos], &line[pos + 9..]);
                    out.push(cleaned);
                } else if let Some(pos) = line.find("override\t") {
                    let cleaned = format!("{}{}", &line[..pos], &line[pos + 9..]);
                    out.push(cleaned);
                } else {
                    out.push(line.to_string());
                }
            }
            out.join("\n")
        }
        "cpp" => {
            let mut s = text.to_string();
            if let Some(pos) = s.find(" override") {
                s.replace_range(pos..pos + 9, "");
            }
            if let Some(pos) = s.find(" final") {
                s.replace_range(pos..pos + 6, "");
            }
            s
        }
        _ => text.to_string(),
    }
}

/// Adjust indentation of a multi-line member text to match `target_indent`.
pub fn adjust_indentation(text: &str, target_indent: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return String::new();
    }

    // Determine the base indentation of the first non-empty line
    let first_non_empty = lines.iter().find(|l| !l.trim().is_empty());
    let source_base_indent_len = first_non_empty
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(0);

    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        if line.trim().is_empty() {
            result.push(String::new());
            continue;
        }
        let cur_indent_len = line.len() - line.trim_start().len();
        let rel_indent_len = cur_indent_len.saturating_sub(source_base_indent_len);
        let extra_spaces = " ".repeat(rel_indent_len);
        result.push(format!(
            "{target_indent}{extra_spaces}{}",
            line.trim_start()
        ));
    }
    result.join("\n")
}

fn normalized_member_text(member: &MemberDecl, language: &str) -> String {
    let text = strip_override_modifiers(&member.full_text, language).replace("\r\n", "\n");
    let lines = text.lines().collect::<Vec<_>>();
    let common_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|c| *c == ' ' || *c == '\t').count())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            let cut = if line.trim().is_empty() || common_indent == 0 {
                0
            } else {
                line.char_indices()
                    .take_while(|(_, c)| *c == ' ' || *c == '\t')
                    .nth(common_indent - 1)
                    .map_or(0, |(byte, c)| byte + c.len_utf8())
            };
            line[cut..].trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn replace_python_pass(text: &mut String, class: &ClassDecl, replacement: &str) -> bool {
    let body = &text[class.body_start..class.body_end];
    let Some(pass_at) = body.find("pass") else {
        return false;
    };
    let line_start = body[..pass_at].rfind('\n').map_or(0, |i| i + 1);
    if !body[line_start..pass_at].trim().is_empty() {
        return false;
    }
    let line_end = body[pass_at..]
        .find('\n')
        .map_or(body.len(), |offset| pass_at + offset);
    text.replace_range(
        class.body_start + line_start..class.body_start + line_end,
        replacement,
    );
    true
}

fn cpp_member_access(class_text: &str, class: &ClassDecl, member: &MemberDecl) -> &'static str {
    let is_struct = class_text[class.decl_start..].starts_with("struct ");
    let mut access = if is_struct { "public" } else { "private" };
    let body_before = &class_text[class.body_start..member.start_offset.min(class.body_end)];
    for line in body_before.lines() {
        match line.trim() {
            "public:" => access = "public",
            "protected:" => access = "protected",
            "private:" => access = "private",
            _ => {}
        }
    }
    access
}

/// Search workspace for a class by name and language.
pub fn find_class_in_workspace(
    root: &Path,
    class_name: &str,
    language: &str,
) -> Option<(PathBuf, String, ClassDecl)> {
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let p = entry.path();
        let lang = crate::lang::language_id_for_path(p);
        if lang != language {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(p) {
            let classes = parse_classes_in_text(&content, language, p);
            if let Some(c) = classes.into_iter().find(|cls| cls.name == class_name) {
                return Some((p.to_path_buf(), content, c));
            }
        }
    }
    None
}

/// Search workspace for all subclasses of a given superclass.
pub fn find_subclasses_in_workspace(
    root: &Path,
    super_class_name: &str,
    language: &str,
) -> Vec<(PathBuf, String, ClassDecl)> {
    let mut results = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let p = entry.path();
        let lang = crate::lang::language_id_for_path(p);
        if lang != language {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(p) {
            let classes = parse_classes_in_text(&content, language, p);
            for cls in classes {
                if cls.super_names.iter().any(|s| s == super_class_name) {
                    results.push((p.to_path_buf(), content.clone(), cls));
                }
            }
        }
    }
    results
}

// ---------------------------------------------------------------------------
// Pull Up Implementation
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn pull_up_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    class_name: &str,
    target_class_opt: Option<&str>,
    members_to_pull: &[String],
    clean_siblings: bool,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<HierarchyRefactorResult> {
    if members_to_pull.is_empty() {
        bail!("No members specified to pull up");
    }

    let sub_file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let classes_in_sub_file = parse_classes_in_text(&sub_file_text, &language, file);
    let sub_class = classes_in_sub_file
        .iter()
        .find(|c| c.name == class_name)
        .cloned()
        .with_context(|| format!("class '{class_name}' not found in {}", file.display()))?;

    // Determine target superclass
    let super_name = match target_class_opt {
        Some(t) => {
            if !sub_class.super_names.iter().any(|s| s == t) && !force {
                bail!(
                    "class '{class_name}' does not list '{t}' as a superclass (known superclasses: {:?})",
                    sub_class.super_names
                );
            }
            t.to_string()
        }
        None => {
            if sub_class.super_names.is_empty() {
                bail!("class '{class_name}' does not declare any superclass");
            }
            if sub_class.super_names.len() > 1 {
                bail!(
                    "class '{class_name}' has multiple superclasses {:?}; please specify 'target_class'",
                    sub_class.super_names
                );
            }
            sub_class.super_names[0].clone()
        }
    };

    // Locate superclass declaration
    let (super_file, super_file_text, super_class) = if let Some(c) =
        classes_in_sub_file.iter().find(|c| c.name == super_name)
    {
        (file.to_path_buf(), sub_file_text.clone(), c.clone())
    } else {
        find_class_in_workspace(root, &super_name, &language).with_context(|| {
            format!("superclass '{super_name}' not found in workspace for language '{language}'")
        })?
    };

    // Validate members exist in subclass and do NOT collide in superclass
    let mut member_decls_to_move = Vec::new();
    for name in members_to_pull {
        let member = sub_class
            .members
            .iter()
            .find(|m| m.name == *name)
            .cloned()
            .with_context(|| format!("member '{name}' not found in class '{class_name}'"))?;

        if let Some(existing) = super_class.members.iter().find(|m| m.name == *name)
            && !force
        {
            bail!(
                "superclass '{super_name}' already defines member '{name}' (start at offset {})",
                existing.start_offset
            );
        }
        member_decls_to_move.push(member);
    }

    // Prepare moved members text with stripped override modifiers and adjusted indentation
    let mut prepared_members = Vec::new();
    for m in &member_decls_to_move {
        let stripped = strip_override_modifiers(&m.full_text, &language);
        let source = if language == "cpp" {
            format!(
                "{}:\n{stripped}",
                cpp_member_access(&sub_file_text, &sub_class, m)
            )
        } else {
            stripped
        };
        let adjusted = adjust_indentation(&source, &super_class.indent);
        prepared_members.push(adjusted);
    }

    let insert_block = prepared_members.join("\n\n");

    // Overlays to produce: file -> new content
    let mut file_contents: BTreeMap<PathBuf, String> = BTreeMap::new();
    let same_file = file == super_file;

    if same_file {
        let mut modified = sub_file_text.clone();

        // 1. Remove members from subclass (in reverse offset order to keep indices valid)
        let mut sorted_members = member_decls_to_move.clone();
        sorted_members.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

        for m in sorted_members {
            // Include leading or trailing newline in removal
            let mut start = m.start_offset;
            let mut end = m.end_offset;
            if end < modified.len() && modified.as_bytes()[end] == b'\n' {
                end += 1;
            } else if start > 0 && modified.as_bytes()[start - 1] == b'\n' {
                start -= 1;
            }
            modified.replace_range(start..end, "");
        }

        // Check if subclass body became completely empty in Python
        if language == "python" {
            // Re-parse to see if subclass body is empty
            if let Some(updated_sub) = parse_classes_in_text(&modified, &language, file)
                .into_iter()
                .find(|c| c.name == class_name)
            {
                let body_slice = &modified[updated_sub.body_start..updated_sub.body_end];
                if body_slice.trim().is_empty() {
                    let pass_stmt = format!("{}pass\n", updated_sub.indent);
                    modified.insert_str(updated_sub.body_start, &pass_stmt);
                }
            }
        }

        // 2. Insert into superclass body
        // Re-parse classes in modified text to get updated superclass offsets
        let updated_classes = parse_classes_in_text(&modified, &language, file);
        let updated_super = updated_classes
            .into_iter()
            .find(|c| c.name == super_name)
            .with_context(|| format!("cannot re-locate superclass '{super_name}' after edits"))?;

        if language == "python" {
            let body_slice = &modified[updated_super.body_start..updated_super.body_end];
            if body_slice.trim() == "pass" {
                replace_python_pass(&mut modified, &updated_super, &insert_block);
            } else {
                let insert_pos = updated_super.body_end;
                let formatted = format!("\n\n{insert_block}");
                modified.insert_str(insert_pos, &formatted);
            }
        } else {
            // TS / C++ / Swift / Rust: insert before closing brace
            let insert_pos = updated_super.body_end;
            let formatted = format!("\n{insert_block}\n");
            modified.insert_str(insert_pos, &formatted);
        }

        file_contents.insert(file.to_path_buf(), modified);
    } else {
        // Multi-file: sub_file and super_file are distinct
        // 1. Edit sub_file
        let mut modified_sub = sub_file_text.clone();
        let mut sorted_members = member_decls_to_move.clone();
        sorted_members.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

        for m in sorted_members {
            let mut start = m.start_offset;
            let mut end = m.end_offset;
            if end < modified_sub.len() && modified_sub.as_bytes()[end] == b'\n' {
                end += 1;
            } else if start > 0 && modified_sub.as_bytes()[start - 1] == b'\n' {
                start -= 1;
            }
            modified_sub.replace_range(start..end, "");
        }

        if language == "python"
            && let Some(updated_sub) = parse_classes_in_text(&modified_sub, &language, file)
                .into_iter()
                .find(|c| c.name == class_name)
        {
            let body_slice = &modified_sub[updated_sub.body_start..updated_sub.body_end];
            if body_slice.trim().is_empty() {
                let pass_stmt = format!("{}pass\n", updated_sub.indent);
                modified_sub.insert_str(updated_sub.body_start, &pass_stmt);
            }
        }
        file_contents.insert(file.to_path_buf(), modified_sub);

        // 2. Edit super_file
        let mut modified_super = super_file_text.clone();
        if language == "python" {
            let body_slice = &modified_super[super_class.body_start..super_class.body_end];
            if body_slice.trim() == "pass" {
                replace_python_pass(&mut modified_super, &super_class, &insert_block);
            } else {
                let insert_pos = super_class.body_end;
                let formatted = format!("\n\n{insert_block}");
                modified_super.insert_str(insert_pos, &formatted);
            }
        } else {
            let insert_pos = super_class.body_end;
            let formatted = format!("\n{insert_block}\n");
            modified_super.insert_str(insert_pos, &formatted);
        }
        file_contents.insert(super_file.clone(), modified_super);
    }

    // 3. Sibling cleanup if requested
    if clean_siblings {
        let candidate_paths: BTreeSet<PathBuf> =
            find_subclasses_in_workspace(root, &super_name, &language)
                .into_iter()
                .map(|(p, _, _)| p)
                .collect();

        for sib_path in candidate_paths {
            let mut modified_text = file_contents
                .get(&sib_path)
                .cloned()
                .unwrap_or_else(|| std::fs::read_to_string(&sib_path).unwrap_or_default());

            let mut any_changed = false;
            loop {
                let current_classes = parse_classes_in_text(&modified_text, &language, &sib_path);
                let sibling_with_target_member = current_classes.into_iter().find(|c| {
                    c.name != class_name
                        && c.super_names.contains(&super_name)
                        && c.members.iter().any(|sibling_member| {
                            member_decls_to_move.iter().any(|pulled_member| {
                                sibling_member.name == pulled_member.name
                                    && normalized_member_text(sibling_member, &language)
                                        == normalized_member_text(pulled_member, &language)
                            })
                        })
                });

                let Some(sib_class) = sibling_with_target_member else {
                    break;
                };

                let matching_members: Vec<MemberDecl> = sib_class
                    .members
                    .into_iter()
                    .filter(|sibling_member| {
                        member_decls_to_move.iter().any(|pulled_member| {
                            sibling_member.name == pulled_member.name
                                && normalized_member_text(sibling_member, &language)
                                    == normalized_member_text(pulled_member, &language)
                        })
                    })
                    .collect();

                let mut sorted = matching_members;
                sorted.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

                for m in sorted {
                    let mut start = m.start_offset;
                    let mut end = m.end_offset;
                    if end < modified_text.len() && modified_text.as_bytes()[end] == b'\n' {
                        end += 1;
                    } else if start > 0 && modified_text.as_bytes()[start - 1] == b'\n' {
                        start -= 1;
                    }
                    modified_text.replace_range(start..end, "");
                }

                if language == "python"
                    && let Some(updated_sib) =
                        parse_classes_in_text(&modified_text, &language, &sib_path)
                            .into_iter()
                            .find(|c| c.name == sib_class.name)
                {
                    let body_slice = &modified_text[updated_sib.body_start..updated_sib.body_end];
                    if body_slice.trim().is_empty() {
                        let pass_stmt = format!("{}pass\n", updated_sib.indent);
                        modified_text.insert_str(updated_sib.body_start, &pass_stmt);
                    }
                }
                any_changed = true;
            }

            if any_changed {
                file_contents.insert(sib_path, modified_text);
            }
        }
    }

    // Prepare unified diff and overlays
    let mut diff_output = String::new();
    let mut overlays = Vec::new();
    let mut files_modified = Vec::new();

    for (p, new_text) in &file_contents {
        let old_text = std::fs::read_to_string(p).unwrap_or_default();
        let rel_path = p
            .strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .to_string();
        files_modified.push(rel_path.clone());
        overlays.push((p.clone(), new_text.clone()));

        let text_diff = similar::TextDiff::from_lines(&old_text, new_text);
        let patch = text_diff
            .unified_diff()
            .header(&format!("a/{rel_path}"), &format!("b/{rel_path}"))
            .to_string();
        diff_output.push_str(&patch);
    }

    // In-memory analyzer validation
    let reports = crate::diagnostics::validate_texts(remote, root, &overlays, &[]).await?;
    let mut diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    // Optional compiler verification
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

    // Apply if requested
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the refactoring produces analyzer errors; nothing was written:\n  {}",
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&file_contents);
        crate::refactor::apply_workspace_edit(root, &edit)?;
    }

    Ok(HierarchyRefactorResult {
        operation: "pull_up".to_string(),
        source_class: class_name.to_string(),
        target_classes: vec![super_name],
        members: members_to_pull.to_vec(),
        files_modified,
        overlays,
        diff: diff_output,
        applied: apply,
        verified,
        diagnostics,
    })
}

// ---------------------------------------------------------------------------
// Push Down Implementation
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn push_down_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    class_name: &str,
    target_classes_opt: Option<&[String]>,
    members_to_push: &[String],
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<HierarchyRefactorResult> {
    if members_to_push.is_empty() {
        bail!("No members specified to push down");
    }

    let super_file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let classes_in_super_file = parse_classes_in_text(&super_file_text, &language, file);
    let super_class = classes_in_super_file
        .iter()
        .find(|c| c.name == class_name)
        .cloned()
        .with_context(|| format!("class '{class_name}' not found in {}", file.display()))?;

    // Verify members exist in superclass
    let mut members_to_move = Vec::new();
    for name in members_to_push {
        let member = super_class
            .members
            .iter()
            .find(|m| m.name == *name)
            .cloned()
            .with_context(|| format!("member '{name}' not found in superclass '{class_name}'"))?;
        members_to_move.push(member);
    }

    // Discover target subclasses
    let candidate_subclasses = find_subclasses_in_workspace(root, class_name, &language);
    let target_subclasses: Vec<(PathBuf, String, ClassDecl)> = match target_classes_opt {
        Some(targets) if !targets.is_empty() => {
            let mut matched = Vec::new();
            for t in targets {
                if let Some(c) = candidate_subclasses
                    .iter()
                    .find(|(_, _, cls)| cls.name == *t)
                {
                    matched.push(c.clone());
                } else if !force {
                    bail!(
                        "specified target subclass '{t}' not found as a subclass of '{class_name}'"
                    );
                }
            }
            matched
        }
        _ => {
            if candidate_subclasses.is_empty() && !force {
                bail!("no subclasses of '{class_name}' found in workspace to push down to");
            }
            candidate_subclasses
        }
    };

    let target_class_names: Vec<String> = target_subclasses
        .iter()
        .map(|(_, _, c)| c.name.clone())
        .collect();

    // Check collisions in target subclasses
    for (_, _, sub) in &target_subclasses {
        for m in members_to_push {
            if let Some(existing) = sub.members.iter().find(|mem| mem.name == *m)
                && !force
            {
                bail!(
                    "target subclass '{}' already defines member '{}'",
                    sub.name,
                    existing.name
                );
            }
        }
    }

    // Track overlays
    let mut file_contents: BTreeMap<PathBuf, String> = BTreeMap::new();

    // 1. Remove members from superclass
    let mut modified_super = super_file_text.clone();
    let mut sorted_members = members_to_move.clone();
    sorted_members.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

    for m in sorted_members {
        let mut start = m.start_offset;
        let mut end = m.end_offset;
        if end < modified_super.len() && modified_super.as_bytes()[end] == b'\n' {
            end += 1;
        } else if start > 0 && modified_super.as_bytes()[start - 1] == b'\n' {
            start -= 1;
        }
        modified_super.replace_range(start..end, "");
    }

    if language == "python"
        && let Some(updated_super) = parse_classes_in_text(&modified_super, &language, file)
            .into_iter()
            .find(|c| c.name == class_name)
    {
        let body_slice = &modified_super[updated_super.body_start..updated_super.body_end];
        if body_slice.trim().is_empty() {
            let pass_stmt = format!("{}pass\n", updated_super.indent);
            modified_super.insert_str(updated_super.body_start, &pass_stmt);
        }
    }
    file_contents.insert(file.to_path_buf(), modified_super);

    // 2. Insert members into each target subclass
    for (sub_path, sub_content, sub_class) in target_subclasses {
        let current_text = file_contents.get(&sub_path).cloned().unwrap_or(sub_content);

        // Adjust member indentation for this subclass
        let mut prepared = Vec::new();
        for m in &members_to_move {
            let source = if language == "cpp" {
                format!(
                    "{}:\n{}",
                    cpp_member_access(&super_file_text, &super_class, m),
                    m.full_text
                )
            } else {
                m.full_text.clone()
            };
            let adjusted = adjust_indentation(&source, &sub_class.indent);
            prepared.push(adjusted);
        }
        let insert_block = prepared.join("\n\n");

        let mut modified_sub = current_text;
        // Re-parse subclass in current text to get latest offsets
        let current_classes = parse_classes_in_text(&modified_sub, &language, &sub_path);
        if let Some(cur_sub) = current_classes
            .into_iter()
            .find(|c| c.name == sub_class.name)
        {
            if language == "python" {
                let body_slice = &modified_sub[cur_sub.body_start..cur_sub.body_end];
                if body_slice.trim() == "pass" {
                    replace_python_pass(&mut modified_sub, &cur_sub, &insert_block);
                } else {
                    let insert_pos = cur_sub.body_end;
                    let formatted = format!("\n\n{insert_block}");
                    modified_sub.insert_str(insert_pos, &formatted);
                }
            } else {
                let insert_pos = cur_sub.body_end;
                let formatted = format!("\n{insert_block}\n");
                modified_sub.insert_str(insert_pos, &formatted);
            }
            file_contents.insert(sub_path, modified_sub);
        }
    }

    // Build unified diff and overlays
    let mut diff_output = String::new();
    let mut overlays = Vec::new();
    let mut files_modified = Vec::new();

    for (p, new_text) in &file_contents {
        let old_text = std::fs::read_to_string(p).unwrap_or_default();
        let rel_path = p
            .strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .to_string();
        files_modified.push(rel_path.clone());
        overlays.push((p.clone(), new_text.clone()));

        let text_diff = similar::TextDiff::from_lines(&old_text, new_text);
        let patch = text_diff
            .unified_diff()
            .header(&format!("a/{rel_path}"), &format!("b/{rel_path}"))
            .to_string();
        diff_output.push_str(&patch);
    }

    // In-memory analyzer validation
    let reports = crate::diagnostics::validate_texts(remote, root, &overlays, &[]).await?;
    let mut diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    // Optional compiler verification
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

    // Apply if requested
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the refactoring produces analyzer errors; nothing was written:\n  {}",
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&file_contents);
        crate::refactor::apply_workspace_edit(root, &edit)?;
    }

    Ok(HierarchyRefactorResult {
        operation: "push_down".to_string(),
        source_class: class_name.to_string(),
        target_classes: target_class_names,
        members: members_to_push.to_vec(),
        files_modified,
        overlays,
        diff: diff_output,
        applied: apply,
        verified,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_class_parsing_and_pull_up() {
        let py_code = r#"
class Animal:
    pass

class Dog(Animal):
    def bark(self):
        print("woof")

    def eat(self):
        print("eating")
"#;
        let classes = parse_python_classes(py_code, Path::new("test.py"));
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].name, "Animal");
        assert_eq!(classes[1].name, "Dog");
        assert_eq!(classes[1].super_names, vec!["Animal"]);
        assert_eq!(classes[1].members.len(), 2);
        assert_eq!(classes[1].members[0].name, "bark");
        assert_eq!(classes[1].members[1].name, "eat");
    }

    #[test]
    fn ts_class_parsing_and_override_stripping() {
        let ts_code = r#"
export class Animal {
    name: string;
}

export class Dog extends Animal {
    override bark(): string {
        return "woof";
    }
}
"#;
        let classes = parse_ts_classes(ts_code, Path::new("test.ts"), "typescript");
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].name, "Animal");
        assert_eq!(classes[1].name, "Dog");
        assert_eq!(classes[1].super_names, vec!["Animal"]);
        assert_eq!(classes[1].members.len(), 1);
        assert_eq!(classes[1].members[0].name, "bark");
        assert!(classes[1].members[0].is_override);

        let stripped = strip_override_modifiers(&classes[1].members[0].full_text, "typescript");
        assert!(!stripped.contains("override"));
        assert!(stripped.contains("bark(): string"));
    }

    #[test]
    fn cpp_class_parsing() {
        let cpp_code = r#"
class Base {
public:
    int id;
};

class Derived : public Base {
public:
    void greet() override {
        std::cout << "hello\n";
    }
};
"#;
        let classes = parse_cpp_classes(cpp_code, Path::new("test.cpp"));
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].name, "Base");
        assert_eq!(classes[1].name, "Derived");
        assert_eq!(classes[1].super_names, vec!["Base"]);
        assert_eq!(classes[1].members.len(), 1);
        assert_eq!(classes[1].members[0].name, "greet");
        assert!(classes[1].members[0].is_override);
    }

    #[test]
    fn swift_class_parsing() {
        let swift_code = r#"
class Animal {
    var name: String
}

class Dog: Animal {
    override func speak() {
        print("woof")
    }
}
"#;
        let classes = parse_swift_classes(swift_code, Path::new("test.swift"));
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].name, "Animal");
        assert_eq!(classes[1].name, "Dog");
        assert_eq!(classes[1].super_names, vec!["Animal"]);
        assert_eq!(classes[1].members.len(), 1);
        assert_eq!(classes[1].members[0].name, "speak");
        assert!(classes[1].members[0].is_override);
    }

    #[test]
    fn rust_trait_parsing() {
        let rust_code = r#"
pub trait SuperTrait {
    fn base_fn(&self);
}

pub trait SubTrait: SuperTrait {
    fn sub_fn(&self);
    type Item;
}
"#;
        let traits = parse_rust_traits(rust_code, Path::new("test.rs"));
        assert_eq!(traits.len(), 2);
        assert_eq!(traits[0].name, "SuperTrait");
        assert_eq!(traits[1].name, "SubTrait");
        assert_eq!(traits[1].super_names, vec!["SuperTrait"]);
        assert_eq!(traits[1].members.len(), 2);
        assert_eq!(traits[1].members[0].name, "sub_fn");
        assert_eq!(traits[1].members[1].name, "Item");
    }
}
