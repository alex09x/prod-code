//! Moving a whole module to another parent: `a::b` becomes `c::b`, with its file and its
//! submodules.
//!
//! The file (and the directory of its submodules) moves. The `mod b;` declaration, with its
//! attributes and doc comment, goes from the old parent to the new one. Every path the analyzer
//! lists as naming the module is spelled anew: a qualified one gets the new parent's path, a
//! bare one in the old parent is imported, and an import in the new parent that would now clash
//! with the declaration is dropped. Inside the moved file `super::` meant the old parent, so it
//! becomes that parent's absolute path. The whole change is type-checked in one overlay before
//! anything is written, the same shape as [`crate::move_item`].

use crate::move_item::{ModulePath, add_import, drop_import, module_of, parent_module_file};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What a module move did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModuleMove {
    pub module: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub from_module: String,
    pub to_module: String,
    /// Old and new path of every file that moves, relative to the checkout.
    pub moved: Vec<(String, String)>,
    /// Every file this writes, whole: the moved files at their new paths and the files whose
    /// paths or declarations changed.
    pub rewritten: Vec<(String, String)>,
    /// The files that go away: the moved files at their old paths.
    pub removed: Vec<String>,
    /// One line per path or import that was spelled anew.
    pub notes: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl ModuleMove {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "module `{}` moved: {} -> {}\n\n",
            self.module, self.from_module, self.to_module
        );
        for (from, to) in &self.moved {
            out.push_str(&format!("- {from} -> {to}\n"));
        }
        out.push('\n');
        let moved_to: Vec<&str> = self.moved.iter().map(|(_, to)| to.as_str()).collect();
        let mut body = String::new();
        for (path, new_text) in &self.rewritten {
            let rel = display(&self.root, Path::new(path));
            // A moved file is shown as a diff against where it came from.
            let old_path = self
                .moved
                .iter()
                .find(|(_, to)| *to == rel)
                .map(|(from, _)| self.root.join(from))
                .unwrap_or_else(|| PathBuf::from(path));
            let old_text = crate::refactor::text_before_apply(&old_path);
            if moved_to.contains(&rel.as_str()) && old_text == *new_text {
                continue;
            }
            body.push_str(
                &similar::TextDiff::from_lines(&old_text, new_text)
                    .unified_diff()
                    .context_radius(2)
                    .header(
                        &format!("a/{}", display(&self.root, &old_path)),
                        &format!("b/{rel}"),
                    )
                    .to_string(),
            );
        }
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.notes.is_empty() {
            out.push_str("\npaths:\n");
            for note in &self.notes {
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
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make these edits\n"
        });
        out
    }

    /// Writes the move: the new files, the rewritten ones, and the old files removed. Refused
    /// while the analyzer rejects the result, unless `force`.
    pub fn write(&mut self, force: bool) -> Result<()> {
        anyhow::ensure!(
            self.diagnostics.is_empty() || force,
            "the move does not compile ({} error(s)); nothing was written:\n  {}",
            self.diagnostics.len(),
            self.diagnostics.join("\n  ")
        );
        let files: BTreeMap<PathBuf, String> = self
            .rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        let mut edit = crate::signature::whole_file_edit(&files);
        if let Some(changes) = edit
            .get_mut("documentChanges")
            .and_then(|c| c.as_array_mut())
        {
            for old in &self.removed {
                changes.push(serde_json::json!({
                    "kind": "delete",
                    "uri": format!("file://{}", self.root.join(old).display()),
                }));
            }
        }
        crate::refactor::apply_workspace_edit(&self.root, &edit)?;
        // The directories the module left, when nothing else is in them.
        let mut dirs: Vec<PathBuf> = self
            .removed
            .iter()
            .filter_map(|old| self.root.join(old).parent().map(Path::to_path_buf))
            .collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        dirs.dedup();
        for dir in dirs {
            let _ = std::fs::remove_dir(dir);
        }
        self.applied = true;
        Ok(())
    }
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte range of the top-level `mod name;` declaration in `text`, from its doc comment and
/// attributes to the end of its line, and the declaration line itself. `None` when the module is
/// not declared that way.
pub fn declaration(text: &str, name: &str) -> Option<(usize, usize, String)> {
    let mut offset = 0;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_end();
        let decl = t
            .strip_suffix(&format!("mod {name};"))
            .is_some_and(|vis| vis.is_empty() || (vis.starts_with("pub") && vis.ends_with(' ')));
        if decl && !line.starts_with(char::is_whitespace) {
            let mut start = offset;
            for above in lines[..i].iter().rev() {
                let a = above.trim_start();
                if a.starts_with("///") || a.starts_with("#[") {
                    start -= above.len();
                } else {
                    break;
                }
            }
            return Some((start, offset + line.len(), t.to_string()));
        }
        offset += line.len();
    }
    None
}

/// `text` with `block` (a module declaration with its attributes) declared where
/// [`crate::move_item::declare_module`] would put a bare one.
pub fn declare_block(text: &str, name: &str, block: &str) -> String {
    let placed = crate::move_item::declare_module(text, name, false);
    let bare = format!("mod {name};");
    match placed.lines().position(|l| l == bare) {
        Some(at) => {
            let mut lines: Vec<String> = placed.lines().map(str::to_string).collect();
            lines[at] = block.trim_end().to_string();
            let mut out = lines.join("\n");
            out.push('\n');
            out
        }
        None => placed,
    }
}

/// `text` with every `super::` (as a path's head, not the tail of a longer name) made `head::`.
pub fn resolve_super(text: &str, head: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut at = 0;
    for (i, _) in text.match_indices("super::") {
        // Not the tail of a longer name, and not the second `super` of `super::super::`.
        if i < at
            || text[..i].chars().next_back().is_some_and(is_ident)
            || text[..i].ends_with("::")
        {
            continue;
        }
        // `super::super::` reaches past the old parent; it is left for the type check to name.
        if text[i + 7..].starts_with("super::") {
            continue;
        }
        out.push_str(&text[at..i]);
        out.push_str(head);
        out.push_str("::");
        at = i + "super::".len();
        count += 1;
    }
    out.push_str(&text[at..]);
    (out, count)
}

/// How a reference to the module is spelled at `offset` of `text`: whether it stands in a `use`
/// group (`{b, x}`), and where its path begins when it is qualified.
#[derive(Debug, PartialEq)]
pub enum Spelling {
    Bare,
    Grouped,
    Qualified { path_start: usize },
}

pub fn spelling(text: &str, offset: usize) -> Spelling {
    let path_start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c) || *c == ':')
        .last()
        .map_or(offset, |(i, _)| i);
    if path_start < offset {
        return Spelling::Qualified { path_start };
    }
    // A group is `{b, c}` inside a `use`; a `{` that opens a block is not one.
    let statement = text[..offset]
        .rfind([';', '}'])
        .map_or(&text[..offset], |i| &text[i + 1..offset])
        .trim_start();
    let in_use = ["use ", "pub use ", "pub(crate) use "]
        .iter()
        .any(|head| statement.starts_with(head));
    match text[..offset].trim_end().chars().next_back() {
        Some('{') | Some(',') if in_use => Spelling::Grouped,
        _ => Spelling::Bare,
    }
}

/// The file that declares a module whose file is `file`: `src/a.rs` for `src/a/b.rs` and for
/// `src/a/b/mod.rs`.
fn declaring_file(file: &Path) -> Option<PathBuf> {
    let module_file = if file.file_name().is_some_and(|n| n == "mod.rs") {
        file.parent()?.with_extension("rs")
    } else {
        file.to_path_buf()
    };
    parent_module_file(&module_file)
}

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn parent_of(module: &ModulePath) -> ModulePath {
    let mut segments = module.segments.clone();
    segments.pop();
    ModulePath {
        krate: module.krate.clone(),
        segments,
    }
}

/// Moves the module whose file is `file` so that its file becomes `target`, with every path to
/// it. Nothing is written; see [`ModuleMove::write`].
pub async fn move_module(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    target: &Path,
) -> Result<ModuleMove> {
    anyhow::ensure!(file.is_file(), "{} is not a file", display(root, file));
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = &canon(root);
    let file = &canon(file);
    // The target does not exist yet: its directory may not either.
    let target = &match (target.parent(), target.file_name()) {
        (Some(dir), Some(name)) if dir.exists() => canon(dir).join(name),
        _ => target.to_path_buf(),
    };
    anyhow::ensure!(!target.exists(), "{} already exists", display(root, target));
    let (_, from) = module_of(file)?;
    let (_, to) = module_of(target)?;
    let name = from
        .segments
        .last()
        .cloned()
        .context("the crate root is not a module that can move")?;
    anyhow::ensure!(
        to.segments.last() == Some(&name),
        "the module keeps its name `{name}` when it moves; move it, then rename it"
    );
    anyhow::ensure!(from.krate == to.krate, "a module moves within its crate");
    let (from_parent, to_parent) = (parent_of(&from), parent_of(&to));
    anyhow::ensure!(
        from_parent != to_parent,
        "{} is already in {}",
        from.absolute(),
        to_parent.absolute()
    );
    anyhow::ensure!(
        !to_parent.segments.starts_with(&from.segments),
        "a module cannot move into itself"
    );

    let old_parent = declaring_file(file)
        .with_context(|| format!("no module file declares {}", display(root, file)))?;
    let mod_rs = file.file_name().is_some_and(|n| n == "mod.rs");
    let new_file = if mod_rs {
        target.with_extension("").join("mod.rs")
    } else {
        target.to_path_buf()
    };
    let new_parent = declaring_file(&new_file).with_context(|| {
        format!(
            "{} has no parent module file to declare it in (for `src/c/b.rs`, `src/c.rs` or \
             `src/c/mod.rs`); create the parent module first",
            display(root, target)
        )
    })?;
    // The submodules live in the module's directory: `src/a/b/` for both layouts.
    let old_dir = if mod_rs {
        file.parent().map(Path::to_path_buf)
    } else {
        Some(file.with_extension(""))
    }
    .filter(|d| d.is_dir());
    let new_dir = if mod_rs {
        new_file.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        new_file.with_extension("")
    };
    if old_dir.is_some() {
        anyhow::ensure!(
            !new_dir.exists(),
            "{} already exists",
            display(root, &new_dir)
        );
    }

    let parent_text = std::fs::read_to_string(&old_parent)
        .with_context(|| format!("cannot read {}", old_parent.display()))?;
    let (decl_start, decl_end, decl_line) = declaration(&parent_text, &name).with_context(|| {
        format!(
            "{} does not declare `mod {name};` at its top level (an inline `mod {name} {{ … }}` \
             or a `#[path]` module is not moved)",
            display(root, &old_parent)
        )
    })?;
    let block = parent_text[decl_start..decl_end].to_string();
    anyhow::ensure!(
        !block.contains("#[path"),
        "`{name}` is declared with `#[path]`; its file is not where the layout puts it"
    );
    let new_parent_text = std::fs::read_to_string(&new_parent)
        .with_context(|| format!("cannot read {}", new_parent.display()))?;
    anyhow::ensure!(
        declaration(&new_parent_text, &name).is_none(),
        "{} already declares a module `{name}`",
        display(root, &new_parent)
    );

    // Where the analyzer says the module is named, asked before any text moves.
    let line_start = parent_text[..decl_end.saturating_sub(1)]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let decl_line_no = parent_text[..line_start].matches('\n').count() as u32 + 1;
    let name_col = decl_line.rfind(&format!("mod {name};")).unwrap_or(0) as u32 + 5;
    let refs =
        crate::signature::references(remote, root, &old_parent, decl_line_no, name_col).await?;

    // The files that move: the module's own, and everything in its directory.
    let mut moves: Vec<(PathBuf, PathBuf)> = vec![(file.to_path_buf(), new_file.clone())];
    if let Some(dir) = &old_dir {
        for f in files_under(dir) {
            if f == *file {
                continue;
            }
            if let Ok(rel) = f.strip_prefix(dir) {
                moves.push((f.clone(), new_dir.join(rel)));
            }
        }
    }
    let moved_to = |path: &Path| {
        moves
            .iter()
            .find(|(old, _)| old == path)
            .map(|(_, new)| new.clone())
    };

    // Each file's text as it is now, then rewritten at the positions the analyzer gave.
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut notes = Vec::new();
    let mut by_file: BTreeMap<PathBuf, Vec<(u32, u32)>> = BTreeMap::new();
    for (path, l, c) in refs {
        by_file.entry(canon(&path)).or_default().push((l, c));
    }
    for (path, mut positions) in by_file {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok((_, module)) = module_of(&path) else {
            notes.push(format!(
                "{} — outside the ordinary crate layout, not rewritten",
                display(root, &path)
            ));
            continue;
        };
        let new_parent_path = to_parent.spelled_from(&module.krate);
        let new_path = to.spelled_from(&module.krate);
        let mut out = text.clone();
        let (mut bare, mut grouped, mut qualified) = (0, 0, 0);
        positions.sort_unstable();
        for (l, c) in positions.into_iter().rev() {
            let Some(offset) = crate::signature::offset_of(&out, l, c) else {
                continue;
            };
            if !out[offset..].starts_with(name.as_str()) {
                continue;
            }
            match spelling(&out, offset) {
                Spelling::Qualified { path_start } => {
                    out.replace_range(path_start..offset, &format!("{new_parent_path}::"));
                    qualified += 1;
                }
                Spelling::Grouped => grouped += 1,
                Spelling::Bare => bare += 1,
            }
        }
        let shown = display(root, &path);
        if qualified > 0 {
            notes.push(format!(
                "{shown}: {qualified} path(s) now through `{new_parent_path}::{name}`"
            ));
        }
        // A grouped import loses the module and imports it on a line of its own.
        if grouped > 0 {
            let (narrowed, dropped) = drop_import(&out, &name);
            out = narrowed;
            for note in dropped {
                notes.push(format!("{shown}: {note}"));
            }
            if path != new_parent {
                out = add_import(&out, &format!("use {new_path};"));
                notes.push(format!("{shown}: added `use {new_path};`"));
            }
        }
        // In the old parent a bare `b` was its own child; now it is imported.
        if bare > 0 && path == old_parent {
            out = add_import(&out, &format!("use {new_path};"));
            notes.push(format!("{shown}: added `use {new_path};`"));
        }
        texts.insert(path, out);
    }

    // The declaration leaves the old parent and arrives in the new one.
    let old_parent_now = texts
        .get(&old_parent)
        .cloned()
        .unwrap_or_else(|| parent_text.clone());
    let (start, end, _) = declaration(&old_parent_now, &name)
        .context("the declaration was rewritten while its paths were")?;
    let mut old_parent_new = old_parent_now.clone();
    old_parent_new.replace_range(start..end, "");
    texts.insert(old_parent.clone(), old_parent_new);
    let new_parent_now = texts.get(&new_parent).cloned().unwrap_or(new_parent_text);
    // An import of the module in its new parent would now clash with the declaration.
    let (new_parent_now, dropped) = drop_import(&new_parent_now, &name);
    for note in dropped {
        notes.push(format!("{}: {note}", display(root, &new_parent)));
    }
    texts.insert(
        new_parent.clone(),
        declare_block(&new_parent_now, &name, &block),
    );

    // The moved files, at their new paths; `super::` in the module's own file meant the old
    // parent.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut removed = Vec::new();
    for (old, new) in &moves {
        let text = match texts.remove(old) {
            Some(t) => t,
            None => std::fs::read_to_string(old)
                .with_context(|| format!("cannot read {}", old.display()))?,
        };
        let text = if old == file {
            let head = from_parent.spelled_from(&from.krate);
            let (resolved, count) = resolve_super(&text, &head);
            if count > 0 {
                notes.push(format!(
                    "{}: {count} `super::` now `{head}::`",
                    display(root, new)
                ));
            }
            resolved
        } else {
            text
        };
        rewritten.insert(new.clone(), text);
        removed.push(display(root, old));
    }
    for (path, text) in texts {
        let path = moved_to(&path).unwrap_or(path);
        rewritten.entry(path).or_insert(text);
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

    Ok(ModuleMove {
        module: name,
        root: root.to_path_buf(),
        from_module: from.absolute(),
        to_module: to.absolute(),
        moved: moves
            .iter()
            .map(|(old, new)| (display(root, old), display(root, new)))
            .collect(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        removed,
        notes,
        diagnostics,
        applied: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_is_found_with_its_attributes_and_nothing_nested() {
        let text = "pub mod a;\n/// The b module.\n#[cfg(feature = \"x\")]\npub(crate) mod b;\nfn f() {}\nmod inner {\n    mod b;\n}\n";
        let (start, end, line) = declaration(text, "b").unwrap();
        assert_eq!(
            &text[start..end],
            "/// The b module.\n#[cfg(feature = \"x\")]\npub(crate) mod b;\n"
        );
        assert_eq!(line, "pub(crate) mod b;");
        assert!(declaration(text, "c").is_none());
        assert!(declaration("mod bb;\nmod ab;\n", "b").is_none());
        assert!(declaration("    mod b;\n", "b").is_none());
        assert!(declaration("mod b {}\n", "b").is_none());
    }

    #[test]
    fn a_block_is_declared_where_a_bare_one_would_go() {
        let out = declare_block(
            "pub mod x;\n\nfn f() {}\n",
            "b",
            "#[cfg(test)]\npub mod b;\n",
        );
        assert_eq!(out, "pub mod x;\n#[cfg(test)]\npub mod b;\n\nfn f() {}\n");
    }

    #[test]
    fn super_becomes_the_old_parent_but_not_inside_a_longer_name() {
        let (out, n) = resolve_super(
            "use super::helper;\nfn f() { super::x(); my_super::y(); super::super::z(); }\n",
            "crate::a",
        );
        assert_eq!(n, 2);
        assert_eq!(
            out,
            "use crate::a::helper;\nfn f() { crate::a::x(); my_super::y(); super::super::z(); }\n"
        );
    }

    #[test]
    fn a_reference_is_bare_grouped_or_qualified() {
        let t = "use crate::a::b;\nuse crate::a::{b, c};\nfn f() { b::g(); super::a::b::g(); }\n";
        let at = |needle: &str, nth: usize| t.match_indices(needle).nth(nth).unwrap().0;
        assert_eq!(
            spelling(t, at("b;", 0)),
            Spelling::Qualified {
                path_start: at("crate", 0)
            }
        );
        assert_eq!(spelling(t, at("b, c", 0)), Spelling::Grouped);
        assert_eq!(spelling(t, at("b::g", 0)), Spelling::Bare);
        assert_eq!(
            spelling(t, at("b::g", 1)),
            Spelling::Qualified {
                path_start: at("super", 0)
            }
        );
    }

    #[test]
    fn the_declaring_file_and_the_files_below_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("a/b/deep")).unwrap();
        std::fs::write(src.join("lib.rs"), "pub mod a;\n").unwrap();
        std::fs::write(src.join("a.rs"), "pub mod b;\n").unwrap();
        std::fs::write(src.join("a/b/mod.rs"), "pub mod deep;\n").unwrap();
        std::fs::write(src.join("a/b/deep/x.rs"), "").unwrap();
        std::fs::write(src.join("a/b/deep.rs"), "pub mod x;\n").unwrap();
        assert_eq!(
            declaring_file(&src.join("a/b/mod.rs")),
            Some(src.join("a.rs"))
        );
        assert_eq!(declaring_file(&src.join("a.rs")), Some(src.join("lib.rs")));
        let found = files_under(&src.join("a/b"));
        assert_eq!(found.len(), 3, "{found:?}");
        let parent = parent_of(&ModulePath {
            krate: "k".into(),
            segments: vec!["a".into(), "b".into()],
        });
        assert_eq!(parent.segments, vec!["a".to_string()]);
    }
}
