/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::options::OutlineOptions;

/// A Markdown file's headings as an outline: `#` to `######`, the level giving the depth, with
/// front matter and fenced code left out.
pub(crate) fn markdown_outline(text: &str, path: &str, options: &OutlineOptions) -> String {
    let mut out = format!("Outline for {path}:\n");
    let mut fence: Option<&str> = None;
    let trimmed_start = text.trim_start_matches('\u{feff}');
    let (mut front_matter, front_matter_marker) = if trimmed_start.starts_with("---")
        && (trimmed_start[3..].starts_with('\n') || trimmed_start[3..].starts_with("\r\n"))
    {
        (true, "---")
    } else if trimmed_start.starts_with("+++")
        && (trimmed_start[3..].starts_with('\n') || trimmed_start[3..].starts_with("\r\n"))
    {
        (true, "+++")
    } else {
        (false, "")
    };
    let mut headings = 0usize;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if front_matter {
            if i > 0 && line.trim() == front_matter_marker {
                front_matter = false;
            }
            continue;
        }
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
            fence = Some(marker);
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) || !trimmed[level..].starts_with([' ', '\t']) {
            continue;
        }
        let title = trimmed[level..].trim().trim_end_matches('#').trim_end();
        if level > options.max_depth || title.is_empty() {
            continue;
        }
        if let Some(kinds) = &options.kinds
            && !kinds.iter().any(|k| k.eq_ignore_ascii_case("heading"))
        {
            continue;
        }
        headings += 1;
        out.push_str(&format!("  [Heading {level}] {title} (line {})\n", i + 1));
    }
    if headings == 0 {
        out.push_str("  (no headings)");
    }
    out.trim_end().to_string()
}
