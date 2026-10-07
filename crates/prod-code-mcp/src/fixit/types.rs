/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

/// One part of a machine-applicable suggestion: replace `start..end` (bytes) of `file` with
/// `replacement`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub line: u64,
    /// The source line the compiler saw at `line`, to tell a stale suggestion from a live one.
    pub line_text: Option<String>,
    pub replacement: String,
}

/// A suggestion the compiler marked `MachineApplicable`, with every part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub level: String,
    pub code: Option<String>,
    /// The diagnostic's message, then the suggestion's own (`remove the whole `use` item`).
    pub message: String,
    pub edits: Vec<Edit>,
}

/// A fix that was applied, or why it was not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub file: String,
    pub line: u64,
    pub message: String,
    /// None when applied; otherwise why not.
    pub skipped: Option<String>,
}

/// What a fix pass did: the fixes, and the check run again after them.
#[derive(Debug, Clone, Serialize)]
pub struct Fixed {
    pub before: crate::verify::VerifyReport,
    pub outcomes: Vec<Outcome>,
    pub after: Option<crate::verify::VerifyReport>,
    /// For a linter that fixes by itself: the command that did, or why nothing could.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Fixed {
    pub fn render(&self, max_items: usize) -> String {
        let mut out = self.before.render(max_items);
        let applied = self.outcomes.iter().filter(|o| o.skipped.is_none()).count();
        match &self.note {
            Some(note) => out.push_str(&format!("\n{note}\n")),
            None => out.push_str(&format!(
                "\nmachine-applicable fixes: {applied} applied, {} skipped\n",
                self.outcomes.len() - applied
            )),
        }
        for o in &self.outcomes {
            match &o.skipped {
                // A linter's own fix mode rewrites whole files, with no line to name.
                None if o.line == 0 => out.push_str(&format!("  fixed {}\n", o.file)),
                None => out.push_str(&format!("  fixed {}:{}: {}\n", o.file, o.line, o.message)),
                Some(why) => out.push_str(&format!(
                    "  skipped {}:{}: {} ({why})\n",
                    o.file, o.line, o.message
                )),
            }
        }
        if let Some(after) = &self.after {
            out.push_str("\nafter the fixes:\n");
            out.push_str(&after.render(max_items));
        }
        if out.len() > crate::verify::MAX_RENDER_BYTES {
            let truncated =
                crate::verify::truncate_to_boundary(&out, crate::verify::MAX_RENDER_BYTES);
            let mut capped = truncated.to_string();
            capped.push_str("\n[... output truncated to avoid exceeding MCP line limits]\n");
            return capped;
        }
        out
    }

    pub fn ok(&self) -> bool {
        self.after.as_ref().unwrap_or(&self.before).ok()
    }
}
