//! Supertypes (roadmap 7.5): what a type implements, and what a trait requires. The other
//! direction, what implements a trait, is `code_implementations`.
//!
//! rust-analyzer has no LSP type hierarchy, so for Rust the answer is read from what it does
//! have. For a trait, the supertraits are the bounds after the colon in its own header. For a
//! type, the derived traits are read from the `#[derive(…)]` attributes above it, and the
//! written ones from its implementations (`textDocument/implementation` on the type): an
//! `impl Trait for Type` header gives `Trait`, and an inherent `impl Type` is not a supertype.
//! rust-analyzer reports a derive among the implementations too, at the attribute for a
//! built-in derive and at the type's own name for a macro such as serde's, which is why derives
//! are read from the attributes instead.
//!
//! The other languages' servers are asked for their own type hierarchy
//! (`textDocument/prepareTypeHierarchy`, then `typeHierarchy/supertypes`). A server without one
//! gets that said instead of an empty list.

use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// One supertype, and where the relation is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supertype {
    pub name: String,
    /// Written as a derive rather than an impl block.
    pub derived: bool,
    /// Where the impl, the derive or the supertype itself is (1-based); `None` for a supertrait
    /// read from a header.
    pub at: Option<(PathBuf, u32, u32)>,
}

/// What the supertypes are of, which decides how they are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Rust type: the traits it implements.
    Type,
    /// A Rust trait: its supertraits.
    Trait,
    /// Another language, answered by its server's type hierarchy.
    Other,
}

/// The supertypes of one type or trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supertypes {
    pub of: String,
    pub kind: Kind,
    pub list: Vec<Supertype>,
    /// Why there is no answer: the language server has no type hierarchy.
    pub unsupported: Option<String>,
}

impl Supertypes {
    pub fn render(&self, root: &Path) -> String {
        if let Some(why) = &self.unsupported {
            return why.clone();
        }
        let (verb, noun) = match self.kind {
            Kind::Type => ("implements", "trait"),
            Kind::Trait => ("requires", "supertrait"),
            Kind::Other => ("has", "supertype"),
        };
        if self.list.is_empty() {
            return format!("`{}` {verb} no {noun}.", self.of);
        }
        let mut out = format!("`{}` {verb} {} {noun}(s):", self.of, self.list.len());
        for s in &self.list {
            out.push_str(&format!("\n  • {}", s.name));
            if s.derived {
                out.push_str("  (derived)");
            }
            if let Some((path, line, col)) = &s.at {
                let shown = path.strip_prefix(root).unwrap_or(path);
                out.push_str(&format!("  {}:{line}:{col}", shown.display()));
            }
        }
        out
    }
}

/// The identifier around the 1-based character `col` of `line`.
fn word_at(line: &str, col: u32) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let at = (col as usize).checked_sub(1)?;
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    if !chars.get(at).is_some_and(is_word) {
        return None;
    }
    let start = (0..=at).rev().take_while(|i| is_word(&chars[*i])).last()?;
    let end = (at..chars.len())
        .take_while(|i| is_word(&chars[*i]))
        .last()?
        + 1;
    Some(chars[start..end].iter().collect())
}

/// The text of `lines` from `from` (0-based) up to the first `{` or `;`, on one line.
fn header_from(lines: &[&str], from: usize) -> String {
    let mut header = String::new();
    for line in lines.iter().skip(from) {
        match line.find(['{', ';']) {
            Some(end) => {
                header.push_str(&line[..end]);
                break;
            }
            None => {
                header.push_str(line);
                header.push(' ');
            }
        }
    }
    header
}

/// `text` split at `sep` where it is outside `<…>` and `(…)`.
fn split_top(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
    }
    parts.push(current.trim().to_string());
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// `text` after a leading `<…>`, with the brackets balanced.
fn skip_generics(text: &str) -> &str {
    let text = text.trim_start();
    if !text.starts_with('<') {
        return text;
    }
    let mut depth = 0;
    for (i, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return &text[i + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// The trait an impl header implements: `Default` for `impl Default for Cache`, `Into<u8>` for
/// `impl<T> Into<u8> for Wrapper<T>`; `None` for an inherent `impl Cache`.
pub fn impl_trait(header: &str) -> Option<String> {
    let at = header.find("impl")?;
    let rest = skip_generics(&header[at + 4..]);
    let rest = rest.split(" where ").next().unwrap_or(rest);
    let mut depth = 0i32;
    let chars: Vec<(usize, char)> = rest.char_indices().collect();
    for (n, &(i, c)) in chars.iter().enumerate() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && rest[i..].starts_with(" for ") && n > 0 {
            let name = rest[..i].trim();
            return (!name.is_empty()).then(|| name.to_string());
        }
    }
    None
}

/// The supertraits in a trait header: `Send + Sync` in `pub trait Embed: Send + Sync {`, and
/// the bounds on `Self` in its `where` clause, which the Rust Reference counts as supertraits
/// too (`trait Circle where Self: Shape`, #227).
pub fn supertraits(header: &str) -> Vec<String> {
    let Some(at) = header.find("trait ") else {
        return Vec::new();
    };
    let after = &header[at + 6..];
    let name_end = after
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    let rest = skip_generics(&after[name_end..]).trim_start();
    // `where` as a word, not inside a name such as `Somewhere`.
    let keyword = rest.match_indices("where").map(|(i, _)| i).find(|&i| {
        (i == 0 || rest[..i].ends_with(char::is_whitespace))
            && rest[i + 5..].starts_with(char::is_whitespace)
    });
    let (bounds, clause) = match keyword {
        Some(w) => (&rest[..w], &rest[w + "where".len()..]),
        None => (rest, ""),
    };
    let mut out = bounds
        .trim_start()
        .strip_prefix(':')
        .map(|b| split_top(b, '+'))
        .unwrap_or_default();
    for predicate in split_top(clause, ',') {
        if let Some(on_self) = predicate.strip_prefix("Self")
            && let Some(b) = on_self.trim_start().strip_prefix(':')
        {
            for bound in split_top(b, '+') {
                if !out.contains(&bound) {
                    out.push(bound);
                }
            }
        }
    }
    out
}

/// The traits derived by the `#[derive(…)]` attributes directly above the declaration on line
/// `decl` (0-based), each with its 1-based position. An attribute may span lines.
fn derives_above(lines: &[&str], decl: usize) -> Vec<(String, u32, u32)> {
    // The attributes and doc comments of this item: up to the end of the one before it.
    let mut start = decl;
    while start > 0 {
        let above = lines[start - 1].trim();
        if above.is_empty() || above.ends_with('}') || above.ends_with(';') || decl - start >= 40 {
            break;
        }
        start -= 1;
    }
    let mut out = Vec::new();
    let mut inside = false;
    for (n, line) in lines.iter().enumerate().take(decl).skip(start) {
        let mut from = 0;
        if !inside {
            match line.find("derive(") {
                Some(at) if line.trim_start().starts_with("#[") || line[..at].contains("#[") => {
                    inside = true;
                    from = at + "derive(".len();
                }
                _ => continue,
            }
        }
        let body = &line[from..];
        let end = body.find(')');
        let names = &body[..end.unwrap_or(body.len())];
        let mut offset = from;
        for part in names.split(',') {
            let name = part.trim();
            if !name.is_empty() {
                let col = line[..offset + part.find(name).unwrap_or(0)]
                    .chars()
                    .count() as u32
                    + 1;
                out.push((name.to_string(), n as u32 + 1, col));
            }
            offset += part.len() + 1;
        }
        if end.is_some() {
            inside = false;
        }
    }
    out
}

/// The line an impl header starts on, when the location on line `at` (0-based) is in one: at
/// most three lines up, for a header broken over lines.
fn impl_header_start(lines: &[&str], at: usize) -> Option<usize> {
    (at.saturating_sub(3)..=at).rev().find(|i| {
        let line = lines[*i].trim_start();
        line.starts_with("impl") || line.starts_with("unsafe impl")
    })
}

/// Is the declaration on this line a trait?
fn is_trait_decl(line: &str) -> bool {
    let mut rest = line.trim_start();
    for prefix in ["pub(crate) ", "pub(super) ", "pub ", "unsafe ", "auto "] {
        rest = rest.strip_prefix(prefix).unwrap_or(rest);
    }
    rest.starts_with("trait ")
}

/// A location from an LSP answer: a `Location` or a `LocationLink`, as (path, 0-based line,
/// 0-based character).
fn location_of(value: &serde_json::Value) -> Option<(PathBuf, u32, u32)> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))?;
    let at = |k: &str| {
        range
            .pointer(&format!("/start/{k}"))
            .and_then(|v| v.as_u64())
    };
    Some((
        PathBuf::from(crate::remote_fs::uri_to_path(uri)),
        at("line")? as u32,
        at("character")? as u32,
    ))
}

fn locations(value: &serde_json::Value) -> Vec<(PathBuf, u32, u32)> {
    match value {
        serde_json::Value::Array(items) => items.iter().filter_map(location_of).collect(),
        other => location_of(other).into_iter().collect(),
    }
}

/// The supertypes of the type or trait at the 1-based `line`:`character` of `file`.
pub async fn supertypes(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
) -> Result<Supertypes> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {file:?}"))?
        .to_string();
    let position = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
    });
    if file.extension().is_some_and(|e| e == "rs") {
        return rust_supertypes(remote, root, file, line, character, position).await;
    }
    let prepared = execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/prepareTypeHierarchy",
        position,
    )
    .await
    .ok()
    .and_then(|v| v.as_array().and_then(|a| a.first()).cloned());
    let Some(item) = prepared else {
        return Ok(Supertypes {
            of: format!("{}:{line}:{character}", file.display()),
            kind: Kind::Other,
            list: Vec::new(),
            unsupported: Some(format!(
                "No type hierarchy at {}:{line}:{character}: the language server answered none (not every server has one).",
                file.display()
            )),
        });
    };
    let supers = execute_lsp_query(
        remote,
        root,
        file,
        "typeHierarchy/supertypes",
        serde_json::json!({ "item": item }),
    )
    .await?;
    let list = supers
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| Supertype {
            name: s
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("?")
                .to_string(),
            derived: false,
            at: s.get("uri").and_then(|u| u.as_str()).map(|uri| {
                let at = |k: &str| {
                    s.pointer(&format!("/selectionRange/start/{k}"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32
                        + 1
                };
                (
                    PathBuf::from(crate::remote_fs::uri_to_path(uri)),
                    at("line"),
                    at("character"),
                )
            }),
        })
        .collect();
    Ok(Supertypes {
        of: item
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string(),
        kind: Kind::Other,
        list,
        unsupported: None,
    })
}

async fn rust_supertypes(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    position: serde_json::Value,
) -> Result<Supertypes> {
    // The declaration: where the name at the position is defined, or the position itself.
    let definition = execute_lsp_query(remote, root, file, "textDocument/definition", position)
        .await
        .ok()
        .and_then(|v| locations(&v).into_iter().next())
        .unwrap_or((
            file.to_path_buf(),
            line.saturating_sub(1),
            character.saturating_sub(1),
        ));
    let (decl_file, decl_line, decl_col) = definition;
    let text = std::fs::read_to_string(&decl_file).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let decl = lines.get(decl_line as usize).copied().unwrap_or("");
    let of = word_at(decl, decl_col + 1).unwrap_or_else(|| "?".to_string());
    if is_trait_decl(decl) {
        let list = supertraits(&header_from(&lines, decl_line as usize))
            .into_iter()
            .map(|name| Supertype {
                name,
                derived: false,
                at: None,
            })
            .collect();
        return Ok(Supertypes {
            of,
            kind: Kind::Trait,
            list,
            unsupported: None,
        });
    }
    let decl_uri = url::Url::from_file_path(&decl_file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {decl_file:?}"))?
        .to_string();
    let impls = execute_lsp_query(
        remote,
        root,
        &decl_file,
        "textDocument/implementation",
        serde_json::json!({
            "textDocument": { "uri": decl_uri },
            "position": { "line": decl_line, "character": decl_col },
        }),
    )
    .await?;
    let mut list: Vec<Supertype> = derives_above(&lines, decl_line as usize)
        .into_iter()
        .map(|(name, l, c)| Supertype {
            name,
            derived: true,
            at: Some((decl_file.clone(), l, c)),
        })
        .collect();
    for (path, l, c) in locations(&impls) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        if l as usize >= lines.len() {
            continue;
        }
        // A derive, reported at its attribute or at the type's name, is already listed.
        let Some(start) = impl_header_start(&lines, l as usize) else {
            continue;
        };
        if let Some(name) = impl_trait(&header_from(&lines, start))
            && !list.iter().any(|s| s.name == name)
        {
            list.push(Supertype {
                name,
                derived: false,
                at: Some((path.clone(), l + 1, c + 1)),
            });
        }
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Supertypes {
        of,
        kind: Kind::Type,
        list,
        unsupported: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_impl_header_names_its_trait_and_an_inherent_one_none() {
        assert_eq!(
            impl_trait("impl Default for SearchIndexes ").as_deref(),
            Some("Default")
        );
        assert_eq!(
            impl_trait("impl<T: Clone> Into<Vec<T>> for Wrapper<T> where T: Send").as_deref(),
            Some("Into<Vec<T>>")
        );
        assert_eq!(
            impl_trait("unsafe impl Send for Handle").as_deref(),
            Some("Send")
        );
        assert_eq!(impl_trait("impl SearchIndexes "), None);
        assert_eq!(impl_trait("impl<T> Wrapper<T> where T: Fn() -> u8"), None);
        assert_eq!(impl_trait("fn main() {}"), None);
    }

    #[test]
    fn a_trait_header_names_its_supertraits() {
        assert_eq!(supertraits("pub trait Embed: Send "), vec!["Send"]);
        assert_eq!(
            supertraits(
                "trait Store<K: Ord>: Clone + Iterator<Item = (K, u8)> + 'static where K: Send"
            ),
            vec!["Clone", "Iterator<Item = (K, u8)>", "'static"]
        );
        assert!(supertraits("pub trait Plain ").is_empty());
        assert_eq!(
            supertraits("pub trait Circle where Self: Shape "),
            vec!["Shape"]
        );
        assert_eq!(
            supertraits("trait Both: Clone where Self: Shape + Clone, T: Copy, Self: Debug"),
            vec!["Clone", "Shape", "Debug"]
        );
        assert!(supertraits("trait Other where T: Copy ").is_empty());
        assert_eq!(supertraits("trait Near: Somewhere "), vec!["Somewhere"]);
        assert!(supertraits("struct Nope ").is_empty());
        assert!(is_trait_decl("pub(crate) unsafe trait Raw {"));
        assert!(!is_trait_decl("pub struct Traits;"));
    }

    #[test]
    fn a_header_is_joined_up_to_its_brace_and_a_word_is_read_at_a_column() {
        let lines = [
            "impl<T>",
            "    Default for Holder<T>",
            "where T: Default {",
            "}",
        ];
        assert_eq!(
            header_from(&lines, 0),
            "impl<T>     Default for Holder<T> where T: Default "
        );
        assert_eq!(
            word_at("#[derive(Clone, Debug)]", 10).as_deref(),
            Some("Clone")
        );
        assert_eq!(
            word_at("#[derive(Clone, Debug)]", 18).as_deref(),
            Some("Debug")
        );
        assert_eq!(word_at("#[derive(Clone)]", 1), None);
        assert_eq!(word_at("x", 0), None);
        assert_eq!(skip_generics("<A<B>> rest"), " rest");
        assert_eq!(skip_generics("<unclosed"), "");
    }

    #[test]
    fn derives_are_read_from_the_attributes_above_the_declaration() {
        let lines = [
            "}",
            "",
            "/// A report.",
            "#[derive(Debug, Clone,",
            "    serde::Serialize)]",
            "#[serde(tag = \"event\")]",
            "pub enum RunEvent {",
        ];
        assert_eq!(
            derives_above(&lines, 6),
            vec![
                ("Debug".to_string(), 4, 10),
                ("Clone".to_string(), 4, 17),
                ("serde::Serialize".to_string(), 5, 5),
            ]
        );
        assert!(derives_above(&["pub struct Bare;"], 0).is_empty());
        let impls = [
            "impl<T>",
            "    Default",
            "    for Holder<T> {",
            "}",
            "pub enum E {",
        ];
        assert_eq!(impl_header_start(&impls, 2), Some(0));
        assert_eq!(impl_header_start(&impls, 4), None);
    }

    #[test]
    fn the_report_says_what_there_is_and_what_there_is_not() {
        let root = Path::new("/w");
        let st = Supertypes {
            of: "Cache".into(),
            kind: Kind::Type,
            list: vec![
                Supertype {
                    name: "Clone".into(),
                    derived: true,
                    at: Some((PathBuf::from("/w/src/lib.rs"), 1, 10)),
                },
                Supertype {
                    name: "Default".into(),
                    derived: false,
                    at: Some((PathBuf::from("/w/src/lib.rs"), 5, 18)),
                },
            ],
            unsupported: None,
        };
        assert_eq!(
            st.render(root),
            "`Cache` implements 2 trait(s):\n  • Clone  (derived)  src/lib.rs:1:10\n  • Default  src/lib.rs:5:18"
        );
        let none = Supertypes {
            of: "Embed".into(),
            kind: Kind::Trait,
            list: vec![],
            unsupported: None,
        };
        assert_eq!(none.render(root), "`Embed` requires no supertrait.");
        let other = Supertypes {
            kind: Kind::Other,
            ..none.clone()
        };
        assert_eq!(other.render(root), "`Embed` has no supertype.");
        let no_server = Supertypes {
            unsupported: Some("No type hierarchy".into()),
            ..none
        };
        assert_eq!(no_server.render(root), "No type hierarchy");
        let link = serde_json::json!({ "targetUri": "file:///w/a.rs",
            "targetSelectionRange": { "start": { "line": 2, "character": 4 } } });
        assert_eq!(locations(&link), vec![(PathBuf::from("/w/a.rs"), 2, 4)]);
        assert!(locations(&serde_json::Value::Null).is_empty());
    }
}
