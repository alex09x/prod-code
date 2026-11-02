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
    let modes = json!([
        {"required":["steps"],"propertyNames":{"enum":super::short_form::PLAN_FIELDS}},
        {"required":["symbol"],"propertyNames":{"enum":super::short_form::SYMBOL_FIELDS},"properties":{"apply":{"const":false}}},
        {"required":["grep"],"propertyNames":{"enum":super::short_form::SEARCH_FIELDS},"properties":{"apply":{"const":false}}}
    ]);
    let steps = json!({
        "type":"array","minItems":1,"maxItems":16,"items":{
            "type":"object","additionalProperties":false,"required":["id","tool"],
            "properties":{
                "id":{"type":"string","pattern":"^[A-Za-z0-9_]{1,48}$"},
                "tool":{"type":"string","enum":super::plan::TOOLS},
                "args":{"type":"object","description":"resolve:{symbol,path?}; symbols:{query,path?,limit?}; grep:{pattern,path?,limit?}; preview:{edits:[{path,old,new}]}; apply:{files:{\"$ref\":\"preview_step#/files\"}}; verify:{kind:check|lint|test,path,filter for tests,timeout_secs?}. Existing code_* tools retain their arguments. Exact $ref objects can supply values from prior steps."},
                "for_each":{"description":"A reference resolving to an array of at most 8 items; $item references the current item."},
                "when":{"type":"object","additionalProperties":false,"required":["step","status"],"properties":{
                    "step":{"type":"string"},"status":{"enum":["ok","error"]}
                }},
                "on_error":{"enum":["stop","continue"],"default":"stop"}
            }
        }
    });
    McpTool {
        name: "code_workflow".into(),
        description: "Compose code investigation, edit preview, explicitly requested application and narrow verification. Short forms: symbol with include:[body,dependencies,callers], or grep with path/limit. Explicit steps use existing code_* investigation tools or resolve/symbols/grep/preview/apply/verify adapters. Pass typed data with {\"$ref\":\"step_id#/json/pointer\"}; for_each expands up to 8 items using $item references. when tests a prior step's ok/error status. References point backward. Preview is in-memory; apply and verify require top-level apply:true and explicit steps. They cannot use for_each or continue after an error. Every apply is an independent batch: previous writes remain if a later step fails. Fingerprint prechecks detect already-changed files; arbitrary editors can still race after the check. No implicit builds/tests or shell commands. One workspace per plan. outputs selects values; warnings and mutation effects remain visible. Limits: 16 steps, 32 operations, 1 MiB retained values. Response has explicit partial/omitted outcomes. Deadlines bound waiting, not an already-running remote job; effects may be unconfirmed after cancellation. Existing standalone tools remain available.".into(),
        input_schema: json!({
            "type":"object", "additionalProperties":false,
            "oneOf": modes,
            "properties": {
                "apply":{"type":"boolean","default":false,"description":"Explicitly allow apply/verify steps. Does not schedule an edit or a check by itself. Short-form investigations remain read-only."},
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
                "timeout_ms":{"type":"integer","minimum":1,"maximum":120000,"default":30000,"description":"Overall response budget: at most 60000 for investigations, 120000 with apply:true; remote checks may need explicit timeouts and can outlive cancelled waiting."},
                "steps": steps
            }
        }),
    }
}
