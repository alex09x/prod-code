//! Replace inheritance with delegation across polyglot OOP languages (Roadmap 7.1.3).
//!
//! Enforces "Composition over Inheritance" by:
//! 1. Decoupling the subclass from its base class (`extends`, `:`, `(...)`).
//! 2. Introducing an encapsulated private delegate field holding the base class instance.
//! 3. Initializing the delegate in constructors / `__init__` (replacing `super()` calls).
//! 4. Auto-generating forwarding methods for inherited base methods to maintain API compatibility.
//! 5. Stripping invalid `override` modifiers from subclass methods and rewriting internal `super.` calls.
//!
//! Supports Python, TypeScript / JavaScript, C++, and Swift.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::pull_push::{
    ClassDecl, MemberKind, find_class_in_workspace, parse_classes_in_text, strip_override_modifiers,
};

/// Outcome of replacing inheritance with delegation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplaceInheritanceResult {
    pub sub_type: String,
    pub base_type: String,
    pub field_name: String,
    pub forwarded_methods: Vec<String>,
    pub files_modified: Vec<String>,
    #[serde(skip)]
    pub overlays: Vec<(PathBuf, String)>,
    pub diff: String,
    pub applied: bool,
    pub verified: bool,
    pub diagnostics: Vec<String>,
}

impl ReplaceInheritanceResult {
    pub fn render(&self, max_diff_len: usize) -> String {
        let mut out = format!(
            "`{}` — replaced inheritance from `{}` with delegation via `{}`\n",
            self.sub_type, self.base_type, self.field_name
        );
        out.push_str(&format!(
            "- forwarded methods ({}): {}\n",
            self.forwarded_methods.len(),
            if self.forwarded_methods.is_empty() {
                "none".to_string()
            } else {
                self.forwarded_methods.join(", ")
            }
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

/// Core implementation for replacing inheritance with delegation.
#[allow(clippy::too_many_arguments)]
pub async fn replace_inheritance_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    sub_type: &str,
    base_type_opt: Option<&str>,
    field_name_opt: Option<&str>,
    methods_opt: Option<&[String]>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceInheritanceResult> {
    let file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let classes_in_file = parse_classes_in_text(&file_text, &language, file);
    let sub_class = classes_in_file
        .iter()
        .find(|c| c.name == sub_type)
        .cloned()
        .with_context(|| format!("class '{sub_type}' not found in {}", file.display()))?;

    // Determine target base class
    let base_name = match base_type_opt {
        Some(b) => {
            if !sub_class.super_names.iter().any(|s| s == b) && !force {
                bail!(
                    "class '{sub_type}' does not inherit from '{b}' (known base classes: {:?})",
                    sub_class.super_names
                );
            }
            b.to_string()
        }
        None => {
            if sub_class.super_names.is_empty() {
                bail!("class '{sub_type}' does not declare any base class");
            }
            if sub_class.super_names.len() > 1 {
                bail!(
                    "class '{sub_type}' inherits from multiple classes {:?}; please specify 'base_type'",
                    sub_class.super_names
                );
            }
            sub_class.super_names[0].clone()
        }
    };

    // Determine default field name if not specified
    let field_name = match field_name_opt {
        Some(f) => f.to_string(),
        None => match language.as_str() {
            "python" => "_base".to_string(),
            "cpp" => "base_".to_string(),
            _ => "base".to_string(),
        },
    };

    // Locate base class to discover its methods
    let base_class_opt = if let Some(b) = classes_in_file.iter().find(|c| c.name == base_name) {
        Some(b.clone())
    } else {
        find_class_in_workspace(root, &base_name, &language).map(|(_, _, c)| c)
    };

    // Determine methods to forward
    let methods_to_forward: Vec<String> = match methods_opt {
        Some(m) if !m.is_empty() => m.to_vec(),
        _ => {
            if let Some(base_cls) = &base_class_opt {
                base_cls
                    .members
                    .iter()
                    .filter(|m| {
                        m.kind == MemberKind::Method
                            && !m.name.starts_with('_')
                            && m.name != "constructor"
                            && !sub_class.members.iter().any(|sm| sm.name == m.name)
                    })
                    .map(|m| m.name.clone())
                    .collect()
            } else {
                Vec::new()
            }
        }
    };

    // Transform the subclass text
    let transformed_sub_text = match language.as_str() {
        "python" => transform_python(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        "typescript" | "javascript" => transform_typescript(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        "cpp" | "c" => transform_cpp(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        "swift" => transform_swift(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        _ => {
            bail!("replace_inheritance_with_delegation is not supported for language '{language}'")
        }
    };

    let mut file_contents: BTreeMap<PathBuf, String> = BTreeMap::new();
    file_contents.insert(file.to_path_buf(), transformed_sub_text);

    // Build diff and overlays
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

    Ok(ReplaceInheritanceResult {
        sub_type: sub_type.to_string(),
        base_type: base_name,
        field_name,
        forwarded_methods: methods_to_forward,
        files_modified,
        overlays,
        diff: diff_output,
        applied: apply,
        verified,
        diagnostics,
    })
}

// ---------------------------------------------------------------------------
// Python Transformation
// ---------------------------------------------------------------------------

fn transform_python(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    _base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // 1. Remove base from class header
    // e.g. `class Dog(Animal):` -> `class Dog:` or `class Dog(Animal, Other):` -> `class Dog(Other):`
    let sub_slice = &out[sub.decl_start..sub.body_start];
    let class_line_end = sub_slice.find('\n').unwrap_or(sub_slice.len());
    let class_header = &sub_slice[..class_line_end];

    if let Some(open_paren) = class_header.find('(')
        && let Some(close_paren) = class_header.find(')')
    {
        let inside_bases = &class_header[open_paren + 1..close_paren];
            let remaining_bases: Vec<&str> = inside_bases
                .split(',')
                .map(str::trim)
                .filter(|b| *b != base_name && !b.is_empty())
                .collect();

            let new_header = if remaining_bases.is_empty() {
                format!("{}:", class_header[..open_paren].trim_end())
            } else {
                format!(
                    "{}({}):",
                    class_header[..open_paren].trim_end(),
                    remaining_bases.join(", ")
                )
            };

            let header_start = sub.decl_start;
            let header_end = sub.decl_start + class_line_end;
            out.replace_range(header_start..header_end, &new_header);
        }

    // Re-parse to get fresh sub offsets
    let updated_classes = parse_classes_in_text(&out, "python", &sub.file_path);
    let updated_sub = updated_classes
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &updated_sub.indent;
    let mut additions = Vec::new();

    // 2. Check for `__init__`
    let init_method = updated_sub.members.iter().find(|m| m.name == "__init__");
    if let Some(init_decl) = init_method {
        // In existing __init__, replace `super().__init__(...)` with `self.{field_name} = {base_name}(...)`
        let init_slice = &out[init_decl.start_offset..init_decl.end_offset];
        if init_slice.contains("super().__init__(") {
            let replaced_init = init_slice.replace(
                "super().__init__(",
                &format!("self.{field_name} = {base_name}("),
            );
            out.replace_range(init_decl.start_offset..init_decl.end_offset, &replaced_init);
        } else if init_slice.contains(&format!("{base_name}.__init__(self")) {
            let replaced_init = init_slice.replace(
                &format!("{base_name}.__init__(self"),
                &format!("self.{field_name} = {base_name}("),
            );
            out.replace_range(init_decl.start_offset..init_decl.end_offset, &replaced_init);
        } else {
            // Prepend `self.{field_name} = {base_name}()` inside __init__
            if let Some(def_colon) = init_slice.find(':') {
                let insert_idx = init_decl.start_offset + def_colon + 1;
                let assign = format!("\n{indent}    self.{field_name} = {base_name}()");
                out.insert_str(insert_idx, &assign);
            }
        }
    } else {
        // Generate new __init__
        let new_init = format!(
            "{indent}def __init__(self, *args, **kwargs):\n{indent}    self.{field_name} = {base_name}(*args, **kwargs)"
        );
        additions.push(new_init);
    }

    // 3. Generate forwarding methods
    for m in methods {
        let fwd = format!(
            "{indent}def {m}(self, *args, **kwargs):\n{indent}    return self.{field_name}.{m}(*args, **kwargs)"
        );
        additions.push(fwd);
    }

    // Insert additions into subclass body
    if !additions.is_empty() {
        let classes_now = parse_classes_in_text(&out, "python", &sub.file_path);
        let cur_sub = classes_now
            .into_iter()
            .find(|c| c.name == sub.name)
            .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

        let body_slice = &out[cur_sub.body_start..cur_sub.body_end];
        let block = additions.join("\n\n");
        if body_slice.trim() == "pass" {
            let pass_start = cur_sub.body_start + body_slice.find("pass").unwrap_or(0);
            let pass_end = pass_start + 4;
            out.replace_range(pass_start..pass_end, &block);
        } else {
            let insert_pos = cur_sub.body_end;
            let formatted = format!("\n\n{block}");
            out.insert_str(insert_pos, &formatted);
        }
    }

    // 4. Rewrite any remaining `super().` calls inside subclass to `self.{field_name}.`
    let final_classes = parse_classes_in_text(&out, "python", &sub.file_path);
    if let Some(final_sub) = final_classes.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[final_sub.body_start..final_sub.body_end];
        if body_slice.contains("super().") {
            let rewritten_body = body_slice.replace("super().", &format!("self.{field_name}."));
            out.replace_range(final_sub.body_start..final_sub.body_end, &rewritten_body);
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript Transformation
// ---------------------------------------------------------------------------

fn transform_typescript(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // 1. Remove `extends <Base>` from class header
    let sub_slice = &out[sub.decl_start..sub.body_start];
    if let Some(extends_pos) = sub_slice.find("extends") {
        let after_extends = &sub_slice[extends_pos + 7..];
        let base_pos = after_extends
            .find(base_name)
            .with_context(|| format!("cannot find base '{base_name}' in extends clause"))?;
        let mut remove_start = sub.decl_start + extends_pos;
        if remove_start > sub.decl_start && out.as_bytes()[remove_start - 1] == b' ' {
            remove_start -= 1;
        }
        let remove_end = sub.decl_start + extends_pos + 7 + base_pos + base_name.len();
        out.replace_range(remove_start..remove_end, "");
    }

    // Re-parse to get fresh offsets
    let classes_now = parse_classes_in_text(&out, &sub.language, &sub.file_path);
    let cur_sub = classes_now
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &cur_sub.indent;
    let mut additions = Vec::new();

    // 2. Delegate field declaration
    let field_decl = format!("{indent}private {field_name}: {base_name};");
    additions.push(field_decl);

    // 3. Constructor handling
    let sub_body = &out[cur_sub.body_start..cur_sub.body_end];
    if sub_body.contains("constructor(") {
        // If constructor exists, replace `super(` with `this.{field_name} = new {base_name}(`
        let replaced_body =
            sub_body.replace("super(", &format!("this.{field_name} = new {base_name}("));
        out.replace_range(cur_sub.body_start..cur_sub.body_end, &replaced_body);
    } else {
        // Generate constructor
        let new_ctor = format!(
            "{indent}constructor(...args: any[]) {{\n{indent}    this.{field_name} = new {base_name}(...args as any);\n{indent}}}"
        );
        additions.push(new_ctor);
    }

    // 4. Generate forwarding methods
    for m in methods {
        // Check if base method has a parsed signature
        let sig = if let Some(base) = base_class {
            base.members.iter().find(|bm| bm.name == *m)
        } else {
            None
        };

        let fwd = if let Some(base_m) = sig {
            // Extract method header up to `{`
            let m_text = &base_m.full_text;
            let open_brace = m_text.find('{').unwrap_or(m_text.len());
            let header = m_text[..open_brace].trim();
            // Clean modifiers (strip export, override, etc.)
            let clean_header = header.replace("override ", "").replace("override\t", "");
            format!(
                "{indent}{clean_header} {{\n{indent}    return this.{field_name}.{m}(...arguments as any);\n{indent}}}"
            )
        } else {
            format!(
                "{indent}{m}(...args: any[]): any {{\n{indent}    return (this.{field_name} as any).{m}(...args);\n{indent}}}"
            )
        };
        additions.push(fwd);
    }

    // Insert additions before closing brace of subclass
    let classes_updated = parse_classes_in_text(&out, &sub.language, &sub.file_path);
    let final_sub = classes_updated
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let insert_block = additions.join("\n\n");
    let insert_pos = final_sub.body_end;
    let formatted = format!("\n{insert_block}\n");
    out.insert_str(insert_pos, &formatted);

    // 5. Strip `override` from remaining subclass methods
    let classes_final = parse_classes_in_text(&out, &sub.language, &sub.file_path);
    if let Some(sub_after) = classes_final.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[sub_after.body_start..sub_after.body_end];
        let stripped_body = strip_override_modifiers(body_slice, "typescript");
        // Also rewrite `super.` to `this.{field_name}.`
        let rewritten_body = stripped_body.replace("super.", &format!("this.{field_name}."));
        out.replace_range(sub_after.body_start..sub_after.body_end, &rewritten_body);
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// C++ Transformation
// ---------------------------------------------------------------------------

fn transform_cpp(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // Replace selected base constructor initializers before removing inheritance. This also
    // initializes the delegate when the original base has no default constructor.
    let mut constructor_edits = Vec::new();
    for member in sub.members.iter().filter(|member| member.name == sub.name) {
        let rewritten = rewrite_cpp_base_initializer(&member.full_text, base_name, field_name);
        if rewritten != member.full_text {
            constructor_edits.push((member.start_offset, member.end_offset, rewritten));
        }
    }
    for (start, end, replacement) in constructor_edits.into_iter().rev() {
        out.replace_range(start..end, &replacement);
    }

    // 1. Remove only the selected base specifier and preserve every other base.
    let sub_slice = &out[sub.decl_start..sub.body_start];
    let header_open = sub_slice
        .find('{')
        .context("class declaration has no body brace")?;
    let header = &sub_slice[..header_open];
    if let Some(colon_pos) = cpp_inheritance_separator(header) {
        let bases = crate::replace_constructor::split_balanced_commas(&header[colon_pos + 1..]);
        let selected_count = bases
            .iter()
            .filter(|base| cpp_base_matches(base, base_name))
            .count();
        anyhow::ensure!(
            selected_count == 1,
            "cannot uniquely identify selected C++ base `{base_name}`; nothing was written"
        );
        let remaining = bases
            .into_iter()
            .filter(|base| !cpp_base_matches(base, base_name))
            .collect::<Vec<_>>();
        let prefix = header[..colon_pos].trim_end();
        let new_header = if remaining.is_empty() {
            format!("{prefix} {}", &sub_slice[header_open..])
        } else {
            format!(
                "{prefix} : {} {}",
                remaining.join(", "),
                &sub_slice[header_open..]
            )
        };
        out.replace_range(sub.decl_start..sub.body_start, &new_header);
    }

    // Re-parse to get fresh offsets
    let classes_now = parse_classes_in_text(&out, "cpp", &sub.file_path);
    let cur_sub = classes_now
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &cur_sub.indent;
    let mut additions = Vec::new();

    // Put the delegate before data members so its constructor runs before member
    // initializers, matching the former base-subobject initialization order.
    let class_kind = out[cur_sub.decl_start..].starts_with("struct ");
    let restored_access = if class_kind { "public:" } else { "private:" };
    let field_decl =
        format!("\n{indent}private:\n{indent}{base_name} {field_name};\n{indent}{restored_access}");
    out.insert_str(cur_sub.body_start, &field_decl);

    // 3. Generate forwarding methods
    let mut fwd_methods = Vec::new();
    for m in methods {
        let sig = if let Some(base) = base_class {
            base.members.iter().find(|bm| bm.name == *m)
        } else {
            None
        };

        let fwd = if let Some(base_m) = sig {
            let m_text = &base_m.full_text;
            let open_brace = m_text.find('{').unwrap_or(m_text.len());
            let header = m_text[..open_brace].trim();
            let parameter_open = header
                .find('(')
                .context("C++ base method has no parameter list")?;
            let parameter_close = crate::parameter_object::matching_bracket(header, parameter_open)
                .context("C++ base method has an invalid parameter list")?;
            let args = cpp_parameter_names(&header[parameter_open + 1..parameter_close])?;
            let clean_header = header
                .replace("override", "")
                .replace("final", "")
                .replace("virtual ", "")
                .replace("= 0", "")
                .trim()
                .to_string();
            format!(
                "{indent}{clean_header} {{\n{indent}    return {field_name}.{m}({args});\n{indent}}}"
            )
        } else {
            format!(
                "{indent}template <typename... Args>\n{indent}auto {m}(Args&&... args) -> decltype(auto) {{\n{indent}    return {field_name}.{m}(std::forward<Args>(args)...);\n{indent}}}"
            )
        };
        fwd_methods.push(fwd);
    }

    if !fwd_methods.is_empty() {
        additions.push(format!("public:\n{}", fwd_methods.join("\n\n")));
    }

    // Insert additions before closing brace of subclass
    let classes_updated = parse_classes_in_text(&out, "cpp", &sub.file_path);
    let final_sub = classes_updated
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let insert_block = additions.join("\n\n");
    let insert_pos = final_sub.body_end;
    let formatted = format!("\n{insert_block}\n");
    out.insert_str(insert_pos, &formatted);

    // 4. Strip `override` and `final` from remaining subclass methods
    let classes_final = parse_classes_in_text(&out, "cpp", &sub.file_path);
    if let Some(sub_after) = classes_final.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[sub_after.body_start..sub_after.body_end];
        let stripped_body = strip_override_modifiers(body_slice, "cpp");
        let rewritten_body =
            stripped_body.replace(&format!("{base_name}::"), &format!("{field_name}."));
        out.replace_range(sub_after.body_start..sub_after.body_end, &rewritten_body);
    }

    Ok(out)
}

fn cpp_inheritance_separator(header: &str) -> Option<usize> {
    let bytes = header.as_bytes();
    let mut angle_depth = 0usize;
    for (i, byte) in bytes.iter().enumerate() {
        match byte {
            b'<' => angle_depth += 1,
            b'>' => angle_depth = angle_depth.saturating_sub(1),
            b':' if angle_depth == 0
                && bytes.get(i.wrapping_sub(1)) != Some(&b':')
                && bytes.get(i + 1) != Some(&b':') =>
            {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

fn cpp_base_matches(specifier: &str, base_name: &str) -> bool {
    let name = specifier
        .split_whitespace()
        .filter(|part| !matches!(*part, "public" | "protected" | "private" | "virtual"))
        .last()
        .unwrap_or("")
        .split('<')
        .next()
        .unwrap_or("");
    let requested = base_name.split('<').next().unwrap_or(base_name).trim();
    name == requested
        || name.rsplit("::").next() == Some(requested.rsplit("::").next().unwrap_or(requested))
}

fn rewrite_cpp_base_initializer(text: &str, base_name: &str, field_name: &str) -> String {
    let Some(params_open) = text.find('(') else {
        return text.to_string();
    };
    let Some(params_close) = crate::parameter_object::matching_bracket(text, params_open) else {
        return text.to_string();
    };
    let body_open = text[params_close + 1..]
        .find('{')
        .map_or(text.len(), |at| params_close + 1 + at);
    let header = &text[params_close + 1..body_open];
    let Some(colon) = cpp_inheritance_separator(header) else {
        return text.to_string();
    };
    let initializers = crate::replace_constructor::split_balanced_commas(&header[colon + 1..]);
    let mut found = false;
    let rewritten = initializers
        .into_iter()
        .map(|initializer| {
            let init = initializer.trim();
            let name_end = init.find(['(', '{']).unwrap_or(init.len());
            let init_name = init[..name_end].trim();
            if !found
                && (init_name == base_name || init_name.rsplit("::").next() == Some(base_name))
            {
                found = true;
                init.replacen(init_name, field_name, 1)
            } else {
                initializer
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    if !found {
        return text.to_string();
    }
    let colon_global = params_close + 1 + colon;
    format!(
        "{}: {}{}",
        text[..colon_global].trim_end(),
        rewritten,
        &text[body_open..]
    )
}

fn cpp_parameter_names(parameters: &str) -> Result<String> {
    if parameters.trim().is_empty() {
        return Ok(String::new());
    }
    crate::replace_constructor::split_balanced_commas(parameters)
        .into_iter()
        .map(|parameter| {
            let declaration = parameter
                .split_once('=')
                .map_or(parameter.as_str(), |(left, _)| left)
                .trim();
            let candidate = declaration
                .split_whitespace()
                .last()
                .unwrap_or("")
                .trim_start_matches(['*', '&']);
            anyhow::ensure!(
                !candidate.is_empty()
                    && candidate.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !matches!(candidate, "const" | "volatile" | "noexcept"),
                "cannot safely forward unnamed or complex C++ parameter `{parameter}`"
            );
            let tokens = declaration.split_whitespace().count();
            anyhow::ensure!(
                tokens >= 2,
                "cannot safely forward unnamed C++ parameter `{parameter}`"
            );
            Ok(candidate.to_string())
        })
        .collect::<Result<Vec<_>>>()
        .map(|names| names.join(", "))
}

fn swift_forwarding_method(
    declaration: &str,
    expected_name: &str,
    field_name: &str,
    indent: &str,
) -> Result<String> {
    let declaration_func_start = declaration
        .find("func ")
        .context("Swift base method has no `func` declaration")?;
    let body_start = declaration[declaration_func_start..]
        .find('{')
        .map_or(declaration.len(), |offset| declaration_func_start + offset);
    let header = declaration[..body_start].trim();
    let func_start = header
        .find("func ")
        .context("Swift base method header has no `func` declaration")?;
    let open = header[func_start..]
        .find('(')
        .map(|offset| func_start + offset)
        .context("Swift base method has no parameter list")?;
    let close = crate::parameter_object::matching_bracket(header, open)
        .context("Swift base method has an invalid parameter list")?;
    let method_name = header[func_start + "func ".len()..open].trim();
    anyhow::ensure!(
        method_name == expected_name,
        "Swift method signature `{method_name}` does not match `{expected_name}` (header: `{header}`)"
    );
    let mut call_args = Vec::new();
    let raw_parameters = &header[open + 1..close];
    for parameter in crate::replace_constructor::split_balanced_commas(raw_parameters) {
        let parameter = parameter.trim();
        if parameter.is_empty() {
            continue;
        }
        anyhow::ensure!(
            !parameter.contains("..."),
            "cannot safely forward variadic Swift parameter `{parameter}`"
        );
        let (labels, ty) = parameter
            .split_once(':')
            .with_context(|| format!("cannot parse Swift parameter `{parameter}`"))?;
        let names = labels.split_whitespace().collect::<Vec<_>>();
        let local = names.last().copied().unwrap_or("");
        anyhow::ensure!(
            !local.is_empty() && local.chars().all(|c| c.is_alphanumeric() || c == '_'),
            "cannot safely forward Swift parameter `{parameter}`"
        );
        let label = if names.len() > 1 { names[0] } else { local };
        let value = if ty.trim_start().starts_with("inout ") {
            format!("&{local}")
        } else {
            local.to_string()
        };
        call_args.push(if label == "_" {
            value
        } else {
            format!("{label}: {value}")
        });
    }
    let suffix = header[close + 1..].trim();
    let mut call_prefix = String::new();
    if suffix.contains("throws") || suffix.contains("rethrows") {
        call_prefix.push_str("try ");
    }
    if suffix.contains("async") {
        call_prefix.push_str("await ");
    }
    let has_value = suffix
        .split_once("->")
        .map(|(_, result)| result.trim().split_whitespace().next().unwrap_or(""))
        .is_some_and(|result| result != "Void" && result != "()" && !result.is_empty());
    let return_prefix = if has_value { "return " } else { "" };
    let mut clean_header = header.to_string();
    for modifier in ["override ", "final "] {
        clean_header = clean_header.replace(modifier, "");
    }
    Ok(format!(
        "{indent}{clean_header} {{\n{indent}    {return_prefix}{call_prefix}self.{field_name}.{expected_name}({})\n{indent}}}",
        call_args.join(", ")
    ))
}

// ---------------------------------------------------------------------------
// Swift Transformation
// ---------------------------------------------------------------------------

fn transform_swift(
    text: &str,
    sub: &ClassDecl,
    base_name: &str,
    field_name: &str,
    methods: &[String],
    base_class: Option<&ClassDecl>,
) -> Result<String> {
    let mut out = text.to_string();

    // 1. Remove only the selected base while preserving protocol conformances.
    let sub_slice = &out[sub.decl_start..sub.body_start];
    let header_open = sub_slice
        .find('{')
        .context("class declaration has no body brace")?;
    let header = &sub_slice[..header_open];
    if let Some(colon_pos) = header.find(':') {
        let bases = header[colon_pos + 1..]
            .split(',')
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .collect::<Vec<_>>();
        let selected_count = bases
            .iter()
            .filter(|base| swift_base_matches(base, base_name))
            .count();
        anyhow::ensure!(
            selected_count == 1,
            "cannot uniquely identify selected Swift base `{base_name}`; nothing was written"
        );
        let remaining = bases
            .into_iter()
            .filter(|base| !swift_base_matches(base, base_name))
            .collect::<Vec<_>>();
        let prefix = header[..colon_pos].trim_end();
        let new_header = if remaining.is_empty() {
            format!("{prefix} {}", &sub_slice[header_open..])
        } else {
            format!(
                "{prefix}: {} {}",
                remaining.join(", "),
                &sub_slice[header_open..]
            )
        };
        out.replace_range(sub.decl_start..sub.body_start, &new_header);
    }

    // Re-parse to get fresh offsets
    let classes_now = parse_classes_in_text(&out, "swift", &sub.file_path);
    let cur_sub = classes_now
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let indent = &cur_sub.indent;
    let mut additions = Vec::new();

    // 2. Delegate field declaration
    let field_decl = format!("{indent}private let {field_name}: {base_name}");
    additions.push(field_decl);

    // 3. Constructor handling
    let sub_body = &out[cur_sub.body_start..cur_sub.body_end];
    if sub_body.contains("init(") {
        let replaced_body =
            sub_body.replace("super.init(", &format!("self.{field_name} = {base_name}("));
        out.replace_range(cur_sub.body_start..cur_sub.body_end, &replaced_body);
    } else {
        let new_init =
            format!("{indent}init() {{\n{indent}    self.{field_name} = {base_name}()\n{indent}}}");
        additions.push(new_init);
    }

    // 4. Generate forwarding methods
    for m in methods {
        let base_method = base_class
            .and_then(|base| base.members.iter().find(|member| member.name == *m))
            .with_context(|| {
                format!("cannot safely forward Swift method `{m}` without its base signature")
            })?;
        additions.push(swift_forwarding_method(
            &base_method.full_text,
            m,
            field_name,
            indent,
        )?);
    }

    // Insert additions before closing brace of subclass
    let classes_updated = parse_classes_in_text(&out, "swift", &sub.file_path);
    let final_sub = classes_updated
        .into_iter()
        .find(|c| c.name == sub.name)
        .with_context(|| format!("failed to re-locate class '{}'", sub.name))?;

    let insert_block = additions.join("\n\n");
    let insert_pos = final_sub.body_end;
    let formatted = format!("\n{insert_block}\n");
    out.insert_str(insert_pos, &formatted);

    // 5. Strip `override` from remaining subclass methods
    let classes_final = parse_classes_in_text(&out, "swift", &sub.file_path);
    if let Some(sub_after) = classes_final.into_iter().find(|c| c.name == sub.name) {
        let body_slice = &out[sub_after.body_start..sub_after.body_end];
        let stripped_body = strip_override_modifiers(body_slice, "swift");
        let rewritten_body = stripped_body.replace("super.", &format!("self.{field_name}."));
        out.replace_range(sub_after.body_start..sub_after.body_end, &rewritten_body);
    }

    Ok(out)
}

fn swift_base_matches(specifier: &str, base_name: &str) -> bool {
    let specifier = specifier
        .trim()
        .split('<')
        .next()
        .unwrap_or(specifier)
        .trim();
    let base = base_name
        .trim()
        .split('<')
        .next()
        .unwrap_or(base_name)
        .trim();
    specifier == base
        || specifier.rsplit('.').next() == Some(base)
        || specifier.rsplit('.').next() == base.rsplit('.').next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_python_replace_inheritance_with_delegation() {
        let py_code = r#"class List:
    def push(self, item):
        pass

    def pop(self):
        pass

class CustomQueue(List):
    def peek(self):
        return self._items[0]
"#;
        let classes = parse_classes_in_text(py_code, "python", Path::new("test.py"));
        let base = &classes[0];
        let sub = &classes[1];

        let transformed = transform_python(
            py_code,
            sub,
            "List",
            "_base",
            &["push".to_string(), "pop".to_string()],
            Some(base),
        )
        .unwrap();

        assert!(!transformed.contains("class CustomQueue(List):"));
        assert!(transformed.contains("class CustomQueue:"));
        assert!(transformed.contains("self._base = List(*args, **kwargs)"));
        assert!(transformed.contains("def push(self, *args, **kwargs):"));
        assert!(transformed.contains("return self._base.push(*args, **kwargs)"));
        assert!(transformed.contains("def pop(self, *args, **kwargs):"));
        assert!(transformed.contains("return self._base.pop(*args, **kwargs)"));
        assert!(transformed.contains("def peek(self):"));
    }

    #[test]
    fn test_ts_replace_inheritance_with_delegation() {
        let ts_code = r#"export class Animal {
    speak(): string {
        return "...";
    }
}

export class Dog extends Animal {
    override bark(): string {
        return "woof";
    }
}
"#;
        let classes = parse_classes_in_text(ts_code, "typescript", Path::new("test.ts"));
        let base = &classes[0];
        let sub = &classes[1];

        let transformed = transform_typescript(
            ts_code,
            sub,
            "Animal",
            "animal",
            &["speak".to_string()],
            Some(base),
        )
        .unwrap();

        assert!(!transformed.contains("class Dog extends Animal"));
        assert!(transformed.contains("class Dog {"));
        assert!(transformed.contains("private animal: Animal;"));
        assert!(transformed.contains("this.animal = new Animal("));
        assert!(transformed.contains("return this.animal.speak("));
        // override should be stripped from bark
        assert!(!transformed.contains("override bark"));
        assert!(transformed.contains("bark(): string"));
    }
}
