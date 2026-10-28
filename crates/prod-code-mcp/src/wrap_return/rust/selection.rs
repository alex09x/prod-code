/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(super) fn selected_function_name_position(
    symbols: &serde_json::Value,
    line: u32,
    wanted: Option<&str>,
) -> Option<(u32, u32)> {
    fn walk(
        nodes: &[serde_json::Value],
        line: u32,
        wanted: Option<&str>,
        best: &mut Option<(u32, u32, u32)>,
    ) {
        for node in nodes {
            let range = node.get("range").or_else(|| {
                node.get("location")
                    .and_then(|location| location.get("range"))
            });
            let kind = node.get("kind").and_then(|kind| kind.as_u64()).unwrap_or(0);
            let name = node
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or("");
            if matches!(kind, 6 | 12)
                && wanted.is_none_or(|wanted| name == wanted)
                && let Some(range) = range
                && let (Some(start), Some(end)) = (
                    range
                        .pointer("/start/line")
                        .and_then(|value| value.as_u64()),
                    range.pointer("/end/line").and_then(|value| value.as_u64()),
                )
                && (start as u32) < line
                && line <= end as u32 + 1
            {
                let span = end.saturating_sub(start) as u32;
                if best.is_none_or(|(best_span, _, _)| span <= best_span) {
                    let position = node
                        .pointer("/selectionRange/start")
                        .or_else(|| range.get("start"));
                    if let (Some(selected_line), Some(selected_col)) = (
                        position
                            .and_then(|position| position.get("line"))
                            .and_then(|value| value.as_u64()),
                        position
                            .and_then(|position| position.get("character"))
                            .and_then(|value| value.as_u64()),
                    ) {
                        *best = Some((span, selected_line as u32 + 1, selected_col as u32 + 1));
                    }
                }
            }
            if let Some(children) = node
                .get("children")
                .and_then(|children| children.as_array())
            {
                walk(children, line, wanted, best);
            }
        }
    }

    let mut best = None;
    walk(symbols.as_array()?, line, wanted, &mut best);
    best.map(|(_, line, col)| (line, col))
}
