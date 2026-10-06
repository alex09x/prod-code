/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

/// The deepest type hierarchy asked for; a larger depth is read as this.
pub const MAX_DEPTH: usize = 6;

/// One supertype, and where the relation is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supertype {
    pub name: String,
    /// Written as a derive rather than an impl block.
    pub derived: bool,
    /// Where the impl, the derive or the supertype itself is (1-based); `None` for a supertrait
    /// read from a header.
    pub at: Option<(PathBuf, u32, u32)>,
    /// Supertypes of this supertype (when depth > 1).
    pub children: Vec<Supertype>,
    /// Already shown higher in the hierarchy (cycle avoidance).
    pub repeated: bool,
}

impl Supertype {
    pub fn new(name: impl Into<String>, derived: bool, at: Option<(PathBuf, u32, u32)>) -> Self {
        Self {
            name: name.into(),
            derived,
            at,
            children: Vec::new(),
            repeated: false,
        }
    }
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
    pub depth: usize,
    /// Why there is no answer: the language server has no type hierarchy.
    pub unsupported: Option<String>,
}

impl Supertypes {
    /// Every supertype in the hierarchy, at every level.
    pub fn count(&self) -> usize {
        fn count_nodes(nodes: &[Supertype]) -> usize {
            nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
        }
        count_nodes(&self.list)
    }

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
        let mut out = if self.depth > 1 {
            format!(
                "`{}` {verb} {} {noun}(s), {} in all to depth {}:",
                self.of,
                self.list.len(),
                self.count(),
                self.depth
            )
        } else {
            format!("`{}` {verb} {} {noun}(s):", self.of, self.list.len())
        };
        fn render_nodes(out: &mut String, nodes: &[Supertype], root: &Path, indent: usize) {
            for s in nodes {
                out.push('\n');
                out.push_str(&"  ".repeat(indent));
                out.push_str(&format!("• {}", s.name));
                if s.derived {
                    out.push_str("  (derived)");
                }
                if let Some((path, line, col)) = &s.at {
                    let canonical_root =
                        std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
                    let canonical_path =
                        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                    let shown = path
                        .strip_prefix(root)
                        .or_else(|_| canonical_path.strip_prefix(&canonical_root))
                        .or_else(|_| path.strip_prefix(&canonical_root))
                        .or_else(|_| canonical_path.strip_prefix(root))
                        .unwrap_or(path);
                    out.push_str(&format!("  {}:{line}:{col}", shown.display()));
                }
                if s.repeated {
                    out.push_str("  (shown above)");
                } else if !s.children.is_empty() {
                    render_nodes(out, &s.children, root, indent + 1);
                }
            }
        }
        render_nodes(&mut out, &self.list, root, 1);
        out
    }
}
