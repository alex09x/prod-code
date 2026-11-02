/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::protocol::McpTool;
use serde_json::json;

pub(super) fn tool() -> McpTool {
    McpTool {
        name: "code_workflow".into(),
        description: "Compose code investigation in one call. Uses existing navigation/search operations; does not edit files or run builds/tests. Start with 'resolve' (args: symbol, optional path) for a unique structured address, or 'symbols' (args: query, optional path, limit <=8) for bounded candidates. Other tool names are existing code_* investigation tools and retain their arguments and 1-based positions. Pass data as {\"$ref\":\"step_id#/path\"}; use for_each with a reference to an array and {\"$ref\":\"$item#/path\"} in args to expand up to 8 items. A when condition tests an earlier step's ok/error status. References must point backward; plans are validated before running. No workspace switching, apply/fix/run or build_index inside steps. Select outputs to keep only useful context (default: last step); every step still reports status. Errors stop by default; on_error:'continue' permits a recovery branch. The response is JSON text with status, steps, executed_calls and truncated. Partial failures and omitted outputs are explicit. Candidate search is not proof of exhaustive index coverage. Limits: 16 steps, 32 calls, 60 seconds, 1 MiB retained intermediate results. Example: resolve a symbol as id 'target', then code_definition with path:{\"$ref\":\"target#/path\"}, line:{\"$ref\":\"target#/line\"}, character:{\"$ref\":\"target#/character\"}. Existing standalone tools remain available.".into(),
        input_schema: json!({
            "type":"object", "additionalProperties":false,
            "oneOf":[
                {"required":["steps"],"propertyNames":{"enum":super::short_form::PLAN_FIELDS}},
                {"required":["symbol"],"propertyNames":{"enum":super::short_form::SYMBOL_FIELDS}},
                {"required":["grep"],"propertyNames":{"enum":super::short_form::SEARCH_FIELDS}}
            ],
            "properties": {
                "structured":{"type":"boolean","default":false,"description":"Also return the full report as structuredContent for programmatic consumers. Default false avoids duplicating code in both response channels; text always contains the JSON report."},
                "grep":{"type":"string","description":"Short form: search text and expand up to limit distinct enclosing symbol hints into definitions."},
                "limit":{"type":"integer","minimum":1,"maximum":8,"description":"Short search result limit (default 3)."},
                "fixed_strings":{"type":"boolean","description":"Treat short search text literally (default true)."},
                "symbol":{"type":"string","description":"Short form: inspect one unique symbol without writing steps."},
                "path":{"type":"string","description":"Optional file/directory hint for short symbol inspection or text search."},
                "include":{"type":"array","minItems":1,"maxItems":5,"uniqueItems":true,"items":{"enum":["body","dependencies","hover","callers","references"]},"description":"Requested sections. Default dependencies already includes the seed body; choose body alone for just that declaration."},
                "depth":{"type":"integer","minimum":0,"maximum":3,"description":"Dependency depth in short symbol inspection (default 1)."},
                "workspace":{"type":"string","description":"Configured workspace or a sibling Git worktree; selected once for all steps."},
                "outputs":{"type":"array","maxItems":16,"items":{"type":"string"},"description":"Step IDs whose values should reach the agent; defaults to the last step."},
                "max_bytes":{"type":"integer","minimum":4096,"maximum":65536,"default":24000,"description":"JSON report budget; structured:true opts into an additional full structuredContent representation."},
                "timeout_ms":{"type":"integer","minimum":1,"maximum":60000,"default":30000},
                "steps":{"type":"array","minItems":1,"maxItems":16,"items":{
                    "type":"object","additionalProperties":false,"required":["id","tool"],
                    "properties":{
                        "id":{"type":"string","pattern":"^[A-Za-z0-9_]{1,48}$"},
                        "tool":{"type":"string","enum":super::plan::TOOLS},
                        "args":{"type":"object","description":"Existing operation arguments with exact $ref objects for values from prior steps."},
                        "for_each":{"description":"A reference resolving to an array of at most 8 items; $item references the current item."},
                        "when":{"type":"object","additionalProperties":false,"required":["step","status"],"properties":{
                            "step":{"type":"string"},"status":{"enum":["ok","error"]}
                        }},
                        "on_error":{"enum":["stop","continue"],"default":"stop"}
                    }
                }}
            }
        }),
    }
}
