//! Moving a declaration into another module, with the imports that keep it compiling.
//!
//! The analyzer says where the symbol is declared and where it is used. This module decides
//! what the text should become — the item cut from one file and pasted into another, the `use`
//! statements rewritten, the qualified paths requalified — and the whole change is type-checked
//! in one overlay before a byte is written, the same shape as [`crate::signature`] and
//! [`crate::schema`].

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What a move did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Move {
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub from: String,
    pub from_module: String,
    pub to: String,
    pub to_module: String,
    /// The path a file outside the target module imports now, e.g. `prod_code_mcp::fixture::Shape`.
    pub new_path: String,
    pub moved_lines: usize,
    /// Every file this touched, whole, ready to be written.
    pub rewritten: Vec<(String, String)>,
    /// One line per import that was added, rewritten or dropped.
    pub imports: Vec<String>,
    /// References the rewrite did not understand, named rather than guessed at.
    pub left_alone: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    /// The target module did not exist: the file created, and the parent that declares it now.
    pub created: Option<(String, String)>,
}

/// The file that must declare the module `target` would be: `src/lib.rs` or `src/main.rs` for
/// `src/util.rs`, and `src/a.rs` or `src/a/mod.rs` for `src/a/util.rs`. The first that exists.
pub fn parent_module_file(target: &Path) -> Option<PathBuf> {
    let dir = target.parent()?;
    let candidates = if dir.file_name().is_some_and(|n| n == "src") {
        vec![dir.join("lib.rs"), dir.join("main.rs")]
    } else {
        vec![dir.with_extension("rs"), dir.join("mod.rs")]
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// `text` with `mod name;` (or `pub mod name;`) declared after its last top-level `mod` line, or
/// after its leading inner doc comments and attributes when it has none.
pub fn declare_module(text: &str, name: &str, public: bool) -> String {
    let line = format!("{}mod {name};", if public { "pub " } else { "" });
    let lines: Vec<&str> = text.lines().collect();
    let is_mod = |l: &str| {
        let t = l.trim_start();
        (t.starts_with("mod ") || t.starts_with("pub mod ") || t.starts_with("pub(crate) mod "))
            && t.trim_end().ends_with(';')
            && !l.starts_with(char::is_whitespace)
    };
    let at = match lines.iter().rposition(|l| is_mod(l)) {
        Some(i) => i + 1,
        None => lines
            .iter()
            .position(|l| {
                let t = l.trim_start();
                !(t.starts_with("//!") || t.starts_with("#![") || t.is_empty())
            })
            .unwrap_or(lines.len()),
    };
    // Blank lines above the declaration and nothing else (what an item cut from the top of the
    // file leaves behind) go.
    let leading_blank = if lines[..at].iter().all(|l| l.trim().is_empty()) {
        at
    } else {
        0
    };
    let at = at - leading_blank;
    let mut out: Vec<String> = lines[leading_blank..]
        .iter()
        .map(|l| l.to_string())
        .collect();
    out.insert(at, line);
    // One blank line between the declarations and what follows them.
    if lines.iter().rposition(|l| is_mod(l)).is_none()
        && out.get(at + 1).is_some_and(|l| !l.trim().is_empty())
    {
        out.insert(at + 1, String::new());
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

impl Move {
    /// The report: what moved, what it costs the files that used it, and whether it compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` moved\n\n- from: {} ({})\n- to:   {} ({})\n- callers now import: `{}`\n- {} \
             line(s) moved\n\n",
            self.symbol,
            self.from,
            self.from_module,
            self.to,
            self.to_module,
            self.new_path,
            self.moved_lines
        );
        if let Some((file, parent)) = &self.created {
            out.insert_str(
                out.len() - 1,
                &format!("- {file} is new, declared in {parent}\n"),
            );
        }
        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            let diff = similar::TextDiff::from_lines(&old_text, new_text);
            changed_lines += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed_lines,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.imports.is_empty() {
            out.push_str("\nimports:\n");
            for note in &self.imports {
                out.push_str(&format!("  {note}\n"));
            }
        }
        if !self.left_alone.is_empty() {
            out.push_str("\nleft alone, check these by hand:\n");
            for note in &self.left_alone {
                out.push_str(&format!("  {note}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
            // An unresolved name in the new home is almost always the same story, and it is
            // not one the report should make the reader work out twice.
            if self
                .diagnostics
                .iter()
                .any(|d| d.contains(&self.to) && d.contains("in this scope"))
            {
                out.push_str(&format!(
                    "\nnames it cannot see from `{}`: the item used something private to `{}`. \
                     Move that too, or widen it to `pub(crate)`, and run this again.\n",
                    self.to_module, self.from_module
                ));
            }
        }
        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}

/// Where a file sits in its crate, in the spelling a `use` needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModulePath {
    /// The crate's name as Rust spells it: dashes turned into underscores.
    pub krate: String,
    /// The module segments under the crate root; empty for the root itself.
    pub segments: Vec<String>,
}

impl ModulePath {
    /// How a file belonging to `from_crate` spells this module: `crate::a::b` inside the same
    /// crate, `the_crate::a::b` from outside it.
    pub fn spelled_from(&self, from_crate: &str) -> String {
        let head = if from_crate == self.krate {
            "crate".to_string()
        } else {
            self.krate.clone()
        };
        std::iter::once(head)
            .chain(self.segments.iter().cloned())
            .collect::<Vec<_>>()
            .join("::")
    }

    /// The unambiguous spelling, for a report: always the crate's own name.
    pub fn absolute(&self) -> String {
        std::iter::once(self.krate.clone())
            .chain(self.segments.iter().cloned())
            .collect::<Vec<_>>()
            .join("::")
    }
}

/// The crate directory above `file`, and the module `file` is inside it.
///
/// Only the ordinary layout is understood: `src/lib.rs` and `src/main.rs` are the crate root,
/// `src/a.rs` and `src/a/mod.rs` are both the module `a`. A file reached through `#[path]` is
/// not, and the caller is told so rather than moved into the wrong module.
pub fn module_of(file: &Path) -> Result<(PathBuf, ModulePath)> {
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let crate_dir = file
        .ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file())
        .with_context(|| format!("no Cargo.toml above {}", file.display()))?
        .to_path_buf();
    let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml"))
        .with_context(|| format!("cannot read {}", crate_dir.join("Cargo.toml").display()))?;
    let krate = crate_name(&manifest).with_context(|| {
        format!(
            "{} names no package",
            crate_dir.join("Cargo.toml").display()
        )
    })?;

    let rel = file.strip_prefix(crate_dir.join("src")).map_err(|_| {
        anyhow::anyhow!(
            "{} is not under {}/src; only the ordinary crate layout is understood",
            file.display(),
            crate_dir.display()
        )
    })?;
    let mut segments: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let last = segments.pop().unwrap_or_default();
    match last.strip_suffix(".rs") {
        Some("lib") | Some("main") | Some("mod") => {}
        Some(stem) => segments.push(stem.to_string()),
        None => anyhow::bail!("{} is not a Rust source file", file.display()),
    }
    Ok((crate_dir, ModulePath { krate, segments }))
}

/// The crate's Rust-spelled name: `[lib] name` when it has one, else `[package] name`.
fn crate_name(manifest: &str) -> Option<String> {
    let mut package = None;
    let mut lib = None;
    let mut section = "";
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']'].as_slice());
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match section {
            "package" => package = Some(value),
            "lib" => lib = Some(value),
            _ => {}
        }
    }
    lib.or(package).map(|n| n.replace('-', "_"))
}

/// The line span (1-based, inclusive) of the smallest declaration containing `line`, and the
/// name the analyzer gives it.
pub fn span_at(symbols: &serde_json::Value, line: u32) -> Option<(String, u32, u32)> {
    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<(String, u32, u32)>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            if let Some(range) = range
                && let (Some(s), Some(e)) = (
                    range.pointer("/start/line").and_then(|l| l.as_u64()),
                    range.pointer("/end/line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                let name = node
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                if s <= line && line <= e && best.as_ref().is_none_or(|(_, bs, be)| e - s < be - bs)
                {
                    *best = Some((name, s, e));
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    best
}

/// The first line of the item once its doc comment and attributes are counted as part of it.
///
/// The analyzer's range starts at `fn`/`struct`; a declaration without its `///` and `#[…]`
/// is a different declaration.
pub fn with_doc_comment(text: &str, start: u32) -> u32 {
    let lines: Vec<&str> = text.lines().collect();
    let mut first = start;
    while first > 1 {
        let above = lines
            .get(first as usize - 2)
            .map(|l| l.trim())
            .unwrap_or("");
        if above.starts_with("///") || above.starts_with("#[") || above.starts_with("//!") {
            first -= 1;
        } else {
            break;
        }
    }
    first
}

/// `text` without lines `start..=end`, and those lines on their own.
///
/// Exactly those lines and no others: the caller adjusts every position below the hole by the
/// number of lines removed, and a cut that also tidied the blank lines around it would make
/// that arithmetic a lie. Whatever blank line is left over is the formatter's business.
pub fn cut(text: &str, start: u32, end: u32) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let (s, e) = (start as usize - 1, end as usize);
    let item = lines[s.min(lines.len())..e.min(lines.len())].join("\n");
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..s.min(lines.len())]);
    kept.extend_from_slice(&lines[e.min(lines.len())..]);
    let mut out = kept.join("\n");
    out.push('\n');
    (out, item)
}

/// `item` appended to `text`, with exactly one blank line between them.
pub fn append_item(text: &str, item: &str) -> String {
    let mut out = text.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(item.trim_end());
    out.push('\n');
    out
}

/// A `use` statement's full text, from `use` to the `;`, and where it ends.
fn use_statements(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    for (offset, _) in text.match_indices("use ") {
        if offset < at {
            continue;
        }
        // Only a statement in column zero, so `pub use` counts while an indented `use` — one
        // inside a function body or a test module — is somebody else's import, not the file's.
        let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let before = &text[line_start..offset];
        if !before.is_empty()
            && before != "pub "
            && !(before.starts_with("pub(") && before.ends_with(") "))
        {
            continue;
        }
        let Some(semi) = text[offset..].find(';') else {
            continue;
        };
        out.push((line_start, offset + semi + 1));
        at = offset + semi + 1;
    }
    out
}

/// Takes `name` out of every `use` statement in `text` that brings it in.
///
/// Returns the new text and what it did, so the report can say it. A grouped import keeps its
/// other names; a statement that imported nothing else goes away with its line.
pub fn drop_import(text: &str, name: &str) -> (String, Vec<String>) {
    let mut out = text.to_string();
    let mut notes = Vec::new();
    for _ in 0..64 {
        let Some((start, end)) = use_statements(&out).into_iter().find(|(s, e)| {
            let stmt = &out[*s..*e];
            leaf_names(stmt).iter().any(|leaf| leaf == name)
        }) else {
            break;
        };
        let stmt = out[start..end].to_string();
        let leaves = leaf_names(&stmt);
        let replacement = if leaves.len() <= 1 {
            notes.push(format!("dropped `{}`", stmt.trim()));
            String::new()
        } else {
            let shrunk = remove_from_group(&stmt, name);
            notes.push(format!("narrowed `{}` to `{}`", stmt.trim(), shrunk.trim()));
            shrunk
        };
        let mut tail_end = end;
        if replacement.is_empty() {
            // Take the newline with the line, or an empty line is left behind.
            if out[tail_end..].starts_with('\n') {
                tail_end += 1;
            }
        }
        out.replace_range(start..tail_end, &replacement);
    }
    (out, notes)
}

/// The names a `use` statement actually brings into scope.
fn leaf_names(stmt: &str) -> Vec<String> {
    let Some(at) = stmt.find("use ") else {
        return Vec::new();
    };
    let body = stmt[at + 4..].trim().trim_end_matches(';').trim();
    match body.split_once('{') {
        None => body
            .rsplit("::")
            .next()
            .map(|leaf| vec![leaf.trim().to_string()])
            .unwrap_or_default(),
        Some((_, group)) => group
            .trim_end_matches('}')
            .split(',')
            .map(|item| {
                item.split(" as ")
                    .next()
                    .unwrap_or(item)
                    .rsplit("::")
                    .next()
                    .unwrap_or(item)
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect(),
    }
}

/// The same statement without `name` in its group.
fn remove_from_group(stmt: &str, name: &str) -> String {
    let Some((head, rest)) = stmt.split_once('{') else {
        return stmt.to_string();
    };
    let (group, tail) = rest.rsplit_once('}').unwrap_or((rest, ""));
    let kept: Vec<&str> = group
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty() && item.split(" as ").next().unwrap_or(item) != name)
        .collect();
    format!("{head}{{{}}}{tail}", kept.join(", "))
}

/// `text` with `use_line` added, after the imports it already has.
pub fn add_import(text: &str, use_line: &str) -> String {
    if text.contains(use_line) {
        return text.to_string();
    }
    let after = use_statements(text)
        .last()
        .map(|(_, end)| *end)
        .or_else(|| {
            // No imports yet: go under the file's own header, not above it.
            let mut at = 0;
            for line in text.lines() {
                let t = line.trim();
                if t.starts_with("//!") || t.starts_with("#![") || t.is_empty() {
                    at += line.len() + 1;
                } else {
                    break;
                }
            }
            Some(at.min(text.len()))
        })
        .unwrap_or(0);
    let mut out = text.to_string();
    let insert = if out[after..].starts_with('\n') {
        format!("\n{use_line}")
    } else {
        format!("{use_line}\n")
    };
    out.insert_str(after, &insert);
    out
}

/// Whether `text` uses `name` as a word of its own, rather than inside a longer identifier.
fn mentions(text: &str, name: &str) -> bool {
    let mut at = 0;
    while let Some(i) = text[at..].find(name) {
        let start = at + i;
        let end = start + name.len();
        let before_ok = start == 0
            || !text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if before_ok && after_ok {
            return true;
        }
        at = end;
    }
    false
}

/// The same statement bringing in only `names`.
fn narrowed(stmt: &str, names: &[String]) -> String {
    let Some((head, _)) = stmt.split_once('{') else {
        return stmt.trim().to_string();
    };
    if names.len() == 1 {
        return format!("{}{};", head.trim_start(), names[0]);
    }
    format!("{}{{{}}};", head.trim_start(), names.join(", "))
}

/// The imports the moved item takes with it.
///
/// An item does not carry its old module's whole header, only the statements that bring in a
/// name the item actually spells — narrowed to those names, so the target gains no import it
/// does not use. What the item needed from a *private* sibling of its old module cannot be
/// carried at all; that is what the type check is for.
pub fn carry_imports(source_text: &str, item: &str, target_text: &str) -> (String, Vec<String>) {
    let mut out = target_text.to_string();
    let mut notes = Vec::new();
    for (start, end) in use_statements(source_text) {
        let stmt = &source_text[start..end];
        let held: Vec<String> = use_statements(&out)
            .into_iter()
            .flat_map(|(s, e)| leaf_names(&out[s..e]))
            .collect();
        let needed: Vec<String> = leaf_names(stmt)
            .into_iter()
            .filter(|leaf| mentions(item, leaf) && !held.contains(leaf))
            .collect();
        if needed.is_empty() {
            continue;
        }
        let line = narrowed(stmt, &needed);
        out = add_import(&out, &line);
        notes.push(format!("carried `{line}`"));
    }
    (out, notes)
}

/// Rewrites `old::path::Name` into `new::path::Name` at the positions the analyzer reported.
///
/// A reference that is spelled bare (`Name`, brought in by a `use`) has nothing to requalify
/// and is left for [`drop_import`] and [`add_import`]; one that is qualified is rewritten in
/// place, from the last position backwards so the earlier offsets stay true.
pub fn requalify(
    text: &str,
    positions: &[(u32, u32)],
    name: &str,
    new_prefix: &str,
) -> (String, usize) {
    let mut out = text.to_string();
    let mut bare = 0;
    let mut sorted: Vec<(u32, u32)> = positions.to_vec();
    sorted.sort_unstable();
    for (line, col) in sorted.into_iter().rev() {
        let Some(offset) = offset_of(&out, line, col) else {
            continue;
        };
        if !out[offset..].starts_with(name) {
            continue;
        }
        let path_start = out[..offset]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == ':')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(offset);
        if path_start == offset {
            bare += 1; // a bare `Name`; it is the import that has to carry it
            continue;
        }
        out.replace_range(path_start..offset, &format!("{new_prefix}::"));
    }
    (out, bare)
}

/// The byte offset of a 1-based line and column.
fn offset_of(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut at = 0;
    for (n, l) in text.lines().enumerate() {
        if n as u32 + 1 == line {
            let mut chars = l.char_indices();
            return Some(
                at + chars
                    .nth(col as usize - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(l.len()),
            );
        }
        at += l.len() + 1;
    }
    None
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Moves the declaration at `file:line:col` into `target`, rewriting the imports of every file
/// that uses it. Nothing is written unless `apply`, and not then if it does not compile.
#[allow(clippy::too_many_arguments)]
pub async fn move_item(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    target: &Path,
    apply: bool,
    force: bool,
) -> Result<Move> {
    anyhow::ensure!(
        file != target,
        "the item is already in {}",
        display(root, target)
    );
    let source_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    // A target that does not exist yet is created, and declared by its parent module (#148).
    let new_module_parent = if target.exists() {
        None
    } else {
        anyhow::ensure!(
            target.extension().is_some_and(|e| e == "rs"),
            "{} is not a Rust source file",
            display(root, target)
        );
        Some(parent_module_file(target).with_context(|| {
            format!(
                "{} does not exist, and neither does a module file to declare it in (for \
                 `src/a/x.rs`, `src/a.rs` or `src/a/mod.rs`); create the parent module first",
                display(root, target)
            )
        })?)
    };
    let target_text = match &new_module_parent {
        Some(_) => String::new(),
        None => std::fs::read_to_string(target)
            .with_context(|| format!("cannot read {}", target.display()))?,
    };

    let (_, from_module) = module_of(file)?;
    let (_, to_module) = module_of(target)?;
    anyhow::ensure!(
        from_module != to_module,
        "{} and {} are the same module",
        display(root, file),
        display(root, target)
    );

    let symbols = document_symbols(remote, root, file).await?;
    let (name, decl_start, decl_end) =
        span_at(&symbols, line).with_context(|| format!("no declaration at line {line}"))?;
    // A method belongs to its `impl`; pasted into a module it is a free function with a
    // `self` it cannot have (#196).
    let decl_offset = crate::signature::offset_of(&source_text, decl_start, 1).unwrap_or(0);
    if let Some((owner, _, _, _)) = crate::extract_field::impl_blocks(&source_text)
        .into_iter()
        .find(|(_, _, open, close)| *open < decl_offset && decl_offset < *close)
    {
        anyhow::bail!(
            "`{name}` is a method of `{owner}`; move it to the type of one of its parameters \
             with `code_move_method` (`prod-code move-method <file> <line> <col> --to-param \
             <name>`)"
        );
    }
    // A `mod x;` line declares a module whose code is in its own file; cutting the line would
    // move nothing of it (#188).
    let module_declaration = format!("mod {name};");
    if source_text
        .lines()
        .skip(decl_start.saturating_sub(1) as usize)
        .take((decl_end + 1).saturating_sub(decl_start) as usize)
        .any(|l| l.trim_end().ends_with(&module_declaration))
    {
        anyhow::bail!(
            "`{name}` is a module with its own file; move the module with `code_move_module` \
             (`prod-code move-module <its file> --to <new file>`)"
        );
    }
    let start = with_doc_comment(&source_text, decl_start);
    let (source_new, item) = cut(&source_text, start, decl_end);
    let (target_with_imports, carried) = carry_imports(&source_text, &item, &target_text);
    let target_new = if target_with_imports.trim().is_empty() {
        format!("{}\n", item.trim())
    } else {
        append_item(&target_with_imports, &item)
    };

    // Ask before the text moves: afterwards the declaration is not where the analyzer left it.
    let refs = crate::signature::references(remote, root, file, line, col).await?;

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), source_new);
    rewritten.insert(target.to_path_buf(), target_new);
    // The analyzer reported these against the file as it is now. In the file the item just
    // left, everything below the hole has moved up by as many lines as the item was long, and
    // a position that is not adjusted points at the wrong text.
    let removed = decl_end - start + 1;
    let mut by_file: BTreeMap<PathBuf, Vec<(u32, u32)>> = BTreeMap::new();
    for (path, l, c) in refs {
        let l = if path == file {
            // A reference inside the item travelled with it.
            if l >= start && l <= decl_end {
                continue;
            }
            if l > decl_end { l - removed } else { l }
        } else {
            l
        };
        by_file.entry(path).or_default().push((l, c));
    }

    let mut imports: Vec<String> = carried
        .into_iter()
        .map(|note| format!("{}: {note}", display(root, target)))
        .collect();
    let mut left_alone = Vec::new();
    for (path, positions) in &by_file {
        if path == target {
            continue; // home already
        }
        let Ok((_, module)) = module_of(path) else {
            left_alone.push(format!(
                "{} — outside the ordinary crate layout, its imports were not touched",
                display(root, path)
            ));
            continue;
        };
        let prefix = to_module.spelled_from(&module.krate);
        let current = rewritten
            .get(path)
            .cloned()
            .unwrap_or_else(|| std::fs::read_to_string(path).unwrap_or_default());
        let (requalified, bare) = requalify(&current, positions, &name, &prefix);
        let (mut text, dropped) = drop_import(&requalified, &name);
        for note in dropped {
            imports.push(format!("{}: {note}", display(root, path)));
        }
        // Only a file that still spells the name on its own needs to import it; one where
        // every use was path-qualified now names the new module inline and needs nothing.
        if bare > 0 {
            let use_line = format!("use {prefix}::{name};");
            text = add_import(&text, &use_line);
            imports.push(format!("{}: added `{use_line}`", display(root, path)));
        }
        rewritten.insert(path.clone(), text);
    }

    // A new module is declared last: the imports above were placed at the positions the
    // analyzer gave, which a line inserted into the parent first would have shifted.
    let mut created = None;
    if let Some(parent) = &new_module_parent {
        let module_name = target
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let public = item.trim_start().starts_with("pub");
        let parent_text = match rewritten.get(parent) {
            Some(text) => text.clone(),
            None => std::fs::read_to_string(parent)
                .with_context(|| format!("cannot read {}", parent.display()))?,
        };
        rewritten.insert(
            parent.clone(),
            declare_module(&parent_text, &module_name, public),
        );
        created = Some((display(root, target), display(root, parent)));
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
    let diagnostics: Vec<String> = reports
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

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the move does not compile ({} error(s)); nothing was written. Fix the request, or \
             pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(Move {
        symbol: name.clone(),
        root: root.to_path_buf(),
        from: display(root, file),
        from_module: from_module.absolute(),
        to: display(root, target),
        to_module: to_module.absolute(),
        new_path: format!("{}::{name}", to_module.absolute()),
        moved_lines: item.lines().count(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        imports,
        left_alone,
        diagnostics,
        applied,
        created,
    })
}

async fn document_symbols(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_module_is_declared_after_the_last_mod_or_at_the_top() {
        let with_mods = "//! Root.\n\npub mod a;\nmod b;\n\npub fn f() {}\n";
        assert_eq!(
            declare_module(with_mods, "util", true),
            "//! Root.\n\npub mod a;\nmod b;\npub mod util;\n\npub fn f() {}\n"
        );
        let without = "//! Root.\n#![allow(dead_code)]\npub fn f() {}\n";
        assert_eq!(
            declare_module(without, "util", false),
            "//! Root.\n#![allow(dead_code)]\nmod util;\n\npub fn f() {}\n"
        );
        // A `mod` inside a block is not the file's own list.
        let nested = "fn f() {\n    mod inner;\n}\n";
        assert!(declare_module(nested, "util", false).starts_with("mod util;\n\nfn f()"));
        // The blank line an item cut from the top leaves behind does not stay above it.
        assert_eq!(
            declare_module("\nuse crate::x;\n", "util", true),
            "pub mod util;\n\nuse crate::x;\n"
        );
    }

    #[test]
    fn a_new_module_s_parent_is_the_crate_root_or_the_directory_s_module() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        std::fs::create_dir_all(src.join("a")).unwrap();
        std::fs::write(src.join("lib.rs"), "").unwrap();
        std::fs::write(src.join("a.rs"), "").unwrap();
        assert_eq!(
            parent_module_file(&src.join("util.rs")),
            Some(src.join("lib.rs"))
        );
        assert_eq!(
            parent_module_file(&src.join("a/util.rs")),
            Some(src.join("a.rs"))
        );
        assert_eq!(parent_module_file(&src.join("b/util.rs")), None);
    }

    #[test]
    fn a_crate_name_comes_from_lib_then_package_and_is_spelled_in_rust() {
        assert_eq!(
            crate_name("[package]\nname = \"prod-code-mcp\"\n").as_deref(),
            Some("prod_code_mcp")
        );
        assert_eq!(
            crate_name("[package]\nname = \"a\"\n\n[lib]\nname = \"b\"\n").as_deref(),
            Some("b")
        );
        assert_eq!(crate_name("[dependencies]\nname = \"x\"\n"), None);
    }

    #[test]
    fn a_module_spells_itself_one_way_inside_its_crate_and_another_outside() {
        let m = ModulePath {
            krate: "prod_code_mcp".into(),
            segments: vec!["fixture".into()],
        };
        assert_eq!(m.spelled_from("prod_code_mcp"), "crate::fixture");
        assert_eq!(m.spelled_from("prod_code_client"), "prod_code_mcp::fixture");
        assert_eq!(m.absolute(), "prod_code_mcp::fixture");
    }

    #[test]
    fn a_doc_comment_and_its_attributes_are_part_of_the_item() {
        let text = "use a;\n\n/// What it does.\n#[inline]\npub fn f() {}\n";
        assert_eq!(with_doc_comment(text, 5), 3);
        let (rest, item) = cut(text, 3, 5);
        assert_eq!(item, "/// What it does.\n#[inline]\npub fn f() {}");
        assert_eq!(rest, "use a;\n\n");
    }

    #[test]
    fn an_import_is_dropped_whole_or_narrowed_to_what_is_left() {
        let (text, notes) = drop_import("use a::b::Name;\nuse c::D;\n", "Name");
        assert_eq!(text, "use c::D;\n");
        assert_eq!(notes.len(), 1);

        let (text, notes) = drop_import("use a::{B, Name, C};\n", "Name");
        assert_eq!(text, "use a::{B, C};\n");
        assert!(notes[0].contains("narrowed"));

        let (text, notes) = drop_import("use a::b::Other;\n", "Name");
        assert_eq!(text, "use a::b::Other;\n");
        assert!(notes.is_empty());
    }

    #[test]
    fn an_import_lands_after_the_ones_that_are_there_and_never_twice() {
        let text = "//! A module.\n\nuse a::B;\nuse c::D;\n\npub fn f() {}\n";
        let once = add_import(text, "use e::F;");
        assert_eq!(
            once,
            "//! A module.\n\nuse a::B;\nuse c::D;\nuse e::F;\n\npub fn f() {}\n"
        );
        assert_eq!(add_import(&once, "use e::F;"), once);
    }

    #[test]
    fn an_import_goes_under_the_header_when_the_file_has_none() {
        let text = "//! A module.\n\npub fn f() {}\n";
        assert_eq!(
            add_import(text, "use e::F;"),
            "//! A module.\n\nuse e::F;\npub fn f() {}\n"
        );
    }

    #[test]
    fn an_import_inside_a_body_is_not_one_of_the_files_imports() {
        let text = "use a::B;\n\nfn f() {\n    use std::io::Write;\n}\n";
        // The new import goes after `use a::B;`, not after the one inside `f`.
        assert_eq!(
            add_import(text, "use e::F;"),
            "use a::B;\nuse e::F;\n\nfn f() {\n    use std::io::Write;\n}\n"
        );
        // And an indented import is not something this file can be relieved of.
        let (out, notes) = drop_import(text, "Write");
        assert_eq!(out, text);
        assert!(notes.is_empty());
    }

    #[test]
    fn an_item_carries_the_imports_it_spells_and_no_others() {
        let source = "use anyhow::{Context, Result};\nuse std::path::Path;\n\nfn f() {}\n";
        let item = "fn moved(p: &Path) -> Result<()> { Ok(()) }";
        let (target, notes) = carry_imports(source, item, "//! Target.\n");
        assert!(target.contains("use anyhow::Result;"), "{target}");
        assert!(target.contains("use std::path::Path;"), "{target}");
        assert!(
            !target.contains("Context"),
            "an import the item does not spell is not carried: {target}"
        );
        assert_eq!(notes.len(), 2);

        // Nothing is carried twice.
        let (again, notes) = carry_imports(source, item, &target);
        assert_eq!(again, target);
        assert!(notes.is_empty());
    }

    #[test]
    fn a_name_inside_a_longer_identifier_is_not_a_mention() {
        assert!(mentions("fn f(p: &Path) {}", "Path"));
        assert!(!mentions("fn f(p: &PathBuf) {}", "Path"));
        assert!(!mentions("let my_path = 1;", "path"));
    }

    #[test]
    fn a_qualified_reference_is_requalified_and_a_bare_one_is_left_to_its_import() {
        let text = "fn g() {\n    crate::old::Name::new();\n    Name::new();\n}\n";
        let (out, bare) = requalify(text, &[(2, 17), (3, 5)], "Name", "crate::new_home");
        assert_eq!(
            out,
            "fn g() {\n    crate::new_home::Name::new();\n    Name::new();\n}\n"
        );
        assert_eq!(
            bare, 1,
            "the bare `Name` is the import's job, not this one's"
        );
    }

    #[test]
    fn the_smallest_declaration_containing_a_line_is_the_one_that_moves() {
        let symbols = serde_json::json!([
            { "name": "Outer", "range": { "start": { "line": 0 }, "end": { "line": 20 } },
              "children": [
                { "name": "inner", "range": { "start": { "line": 4 }, "end": { "line": 8 } } }
              ] }
        ]);
        assert_eq!(span_at(&symbols, 6), Some(("inner".to_string(), 5, 9)));
        assert_eq!(span_at(&symbols, 2), Some(("Outer".to_string(), 1, 21)));
    }
}
