/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;

/// How many bytes of slice to return before stopping, unless the caller says otherwise.
pub const DEFAULT_MAX_BYTES: usize = 24 * 1024;
/// How far to follow dependencies from the seed by default.
pub const DEFAULT_DEPTH: u32 = 2;
/// Distinct names resolved per item; a body that mentions more is reported as a gap.
pub(crate) const MAX_NAMES_PER_ITEM: usize = 64;
/// Names from outside the workspace listed in the report before it says "and N more".
pub(crate) const EXTERNAL_SHOWN: usize = 12;
/// Gaps listed in the report before it says "and N more".
pub(crate) const GAPS_SHOWN: usize = 20;

/// One item in the slice: a whole declaration, as it appears in its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceItem {
    /// Path relative to the workspace root.
    pub file: String,
    pub name: String,
    pub kind: &'static str,
    /// 1-based inclusive line range of the whole declaration.
    pub start_line: u32,
    pub end_line: u32,
    /// How many edges from the seed: 0 is the seed itself.
    pub depth: u32,
    /// Why it is here: the item that referenced it.
    pub because: Option<String>,
    pub text: String,
}

/// Why a dependency of an item has no usable evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    /// The analyzer failed or refused a definition query.
    QueryFailed,
    /// A definition answer that is neither null, a location nor a list of locations, or one
    /// whose coordinates do not fit.
    Malformed,
    /// A definition in the workspace whose file cannot be read or whose symbols cannot be listed.
    Unreadable,
    /// A body that names more distinct names than are resolved per item.
    NameLimit,
}

impl GapKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            GapKind::QueryFailed => "definition query failed",
            GapKind::Malformed => "malformed definition answer",
            GapKind::Unreadable => "target cannot be sliced",
            GapKind::NameLimit => "name limit",
        }
    }
}

/// One dependency lookup that has no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceGap {
    pub kind: GapKind,
    /// The item whose body was being resolved.
    pub item: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct SliceReport {
    pub seed: String,
    pub items: Vec<SliceItem>,
    /// Optional intra-function data-flow and control-dependency slice of the seed.
    pub dataflow_slice: Option<crate::dataflow::DataFlowSlice>,
    /// Bytes of the files the slice draws from.
    pub source_bytes: usize,
    /// Names that resolved outside the workspace (std, crates.io) and were not followed.
    pub external: Vec<String>,
    /// Names that resolved to a URI of another scheme than `file:` (a class in a jar, a
    /// generated document), as `name (scheme:)`; outside the workspace and not readable.
    pub unsupported: Vec<String>,
    /// Names that resolved in the workspace outside any declaration the slicer includes (a
    /// module, a field of no listed item), as `name (file:line)`; not followed.
    pub unsliced: Vec<String>,
    /// Lookups without usable evidence: while any is here, the slice may miss items.
    pub gaps: Vec<SliceGap>,
    /// The depth the walk was asked to follow.
    pub depth_limit: u32,
    /// Items at the depth limit, whose own dependencies were not looked up.
    pub unexpanded: usize,
    /// The byte budget the walk was given.
    pub max_bytes: usize,
    /// The seed alone exceeds the budget; it is returned whole.
    pub seed_over_budget: bool,
    /// Queued items left out because the budget ran out; their dependencies were not looked up.
    pub truncated: usize,
}

/// Configuration options for program slicing.
#[derive(Debug, Clone)]
pub struct SliceOptions {
    pub depth: u32,
    pub max_bytes: usize,
    pub dataflow: bool,
    pub target_line: Option<u32>,
    pub target_var: Option<String>,
}

impl Default for SliceOptions {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            max_bytes: DEFAULT_MAX_BYTES,
            dataflow: false,
            target_line: None,
            target_var: None,
        }
    }
}

impl SliceReport {
    pub fn slice_bytes(&self) -> usize {
        self.items.iter().map(|i| i.text.len()).sum()
    }

    /// Share of the source files the slice replaces, as a percentage.
    pub fn reduction_percent(&self) -> f64 {
        if self.source_bytes == 0 {
            return 0.0;
        }
        100.0 - (self.slice_bytes() as f64 * 100.0 / self.source_bytes as f64)
    }

    /// Whether every dependency lookup of the walk had a usable answer.
    pub fn has_complete_evidence(&self) -> bool {
        self.gaps.is_empty()
    }

    /// Whether the walk stopped before the whole dependency closure: at the depth limit, at the
    /// byte budget, or at a target in the workspace outside any declaration it slices.
    pub fn is_bounded(&self) -> bool {
        self.unexpanded > 0 || self.truncated > 0 || !self.unsliced.is_empty()
    }

    /// Whether the slice is the seed's whole dependency closure in the workspace: every lookup
    /// answered and nothing left behind a bound. Names outside the workspace are never followed
    /// and do not count against it.
    pub fn is_complete(&self) -> bool {
        self.has_complete_evidence() && !self.is_bounded()
    }

    pub fn render(&self) -> String {
        let mut out = if self.is_complete() {
            format!(
                "slice of `{}`: {} item(s), {} bytes from {} bytes of source ({:.0}% smaller)\n",
                self.seed,
                self.items.len(),
                self.slice_bytes(),
                self.source_bytes,
                self.reduction_percent()
            )
        } else if self.has_complete_evidence() {
            format!(
                "BOUNDED slice of `{}`: {} item(s), {} bytes from {} bytes of source ({:.0}% \
                 smaller); every dependency lookup was answered, but the walk stopped at the \
                 bounds below, so it is not the whole dependency closure\n",
                self.seed,
                self.items.len(),
                self.slice_bytes(),
                self.source_bytes,
                self.reduction_percent()
            )
        } else {
            format!(
                "INCOMPLETE slice of `{}`: {} item(s), {} bytes from {} bytes of source; {} \
                 dependency lookup(s) have no usable answer, so items may be missing and no \
                 reduction is claimed\n",
                self.seed,
                self.items.len(),
                self.slice_bytes(),
                self.source_bytes,
                self.gaps.len()
            )
        };
        if self.seed_over_budget {
            out.push_str(&format!(
                "the seed alone is {} bytes, over the byte budget of {} bytes; it is returned whole\n",
                self.items.first().map_or(0, |i| i.text.len()),
                self.max_bytes
            ));
        }
        if self.truncated > 0 {
            out.push_str(&format!(
                "byte budget of {} bytes reached: {} queued item(s) left out, and their own \
                 dependencies were not looked up\n",
                self.max_bytes, self.truncated
            ));
        }
        if self.unexpanded > 0 {
            out.push_str(&format!(
                "depth limit {} reached: the dependencies of {} item(s) at depth {} were not \
                 looked up\n",
                self.depth_limit, self.unexpanded, self.depth_limit
            ));
        }
        push_list(
            &mut out,
            "outside the workspace, not followed",
            &self.external,
        );
        push_list(
            &mut out,
            "outside the workspace in a source of an unsupported URI scheme, not followed",
            &self.unsupported,
        );
        push_list(
            &mut out,
            "outside any declaration the slicer includes, not followed",
            &self.unsliced,
        );
        if !self.gaps.is_empty() {
            out.push_str("missing evidence:\n");
            for gap in self.gaps.iter().take(GAPS_SHOWN) {
                out.push_str(&format!(
                    "  - {} in `{}`: {}\n",
                    gap.kind.label(),
                    gap.item,
                    gap.detail
                ));
            }
            if self.gaps.len() > GAPS_SHOWN {
                out.push_str(&format!("  - and {} more\n", self.gaps.len() - GAPS_SHOWN));
            }
        }
        if let Some(ref df) = self.dataflow_slice {
            out.push('\n');
            out.push_str(&df.formatted_slice);
            out.push('\n');
        }
        let mut by_file: BTreeMap<&str, Vec<&SliceItem>> = BTreeMap::new();
        for item in &self.items {
            by_file.entry(item.file.as_str()).or_default().push(item);
        }
        for (file, mut items) in by_file {
            items.sort_by_key(|i| i.start_line);
            out.push_str(&format!("\n=== {file}\n"));
            for item in items {
                let why = match (&item.because, item.depth) {
                    (_, 0) if self.dataflow_slice.is_some() => {
                        " (the seed, data-flow sliced)".to_string()
                    }
                    (_, 0) => " (the seed)".to_string(),
                    (Some(from), d) => format!(" (depth {d}, used by {from})"),
                    (None, d) => format!(" (depth {d})"),
                };
                out.push_str(&format!(
                    "\n[{}] {}  {}:{}-{}{}\n{}\n",
                    item.kind, item.name, file, item.start_line, item.end_line, why, item.text
                ));
            }
        }
        out
    }
}

/// `title: a, b, c, and N more`, sorted and deduplicated; nothing when `names` is empty.
pub(crate) fn push_list(out: &mut String, title: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let mut names = names.to_vec();
    names.sort();
    names.dedup();
    let shown = names.len().min(EXTERNAL_SHOWN);
    let more = names.len() - shown;
    out.push_str(&format!(
        "{title}: {}{}\n",
        names[..shown].join(", "),
        if more > 0 {
            format!(", and {more} more")
        } else {
            String::new()
        }
    ));
}
