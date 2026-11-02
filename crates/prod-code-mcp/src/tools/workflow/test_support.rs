/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

pub(super) fn report(result: &McpToolCallResult) -> Value {
    let McpContentItem::Text { text } = &result.content[0];
    serde_json::from_str(text).expect("workflow JSON report in the compatible text channel")
}
