/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Operation and compilation metrics Prometheus exposition formatting.

use super::escape_label_value;
use crate::metrics::Metrics;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct OpKey {
    category: String,
    method: String,
    engine: String,
    status: &'static str,
    error_class: String,
}

#[derive(Default)]
struct OpAgg {
    count: u64,
    duration_ms: u64,
    items: u64,
    bytes: u64,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct CompKey {
    engine: String,
    compiler: String,
    status: &'static str,
    error_class: String,
}

#[derive(Default)]
struct CompAgg {
    count: u64,
    duration_ms: u64,
}

pub fn format_operation_metrics(out: &mut String, metrics: &Metrics, node: &str) {
    let mut agg_map: BTreeMap<OpKey, OpAgg> = BTreeMap::new();
    let mut comp_map: BTreeMap<CompKey, CompAgg> = BTreeMap::new();

    metrics.for_each_ring_event(|ev| {
        let status = if ev.ok { "ok" } else { "error" };
        let category = if !ev.kind.is_empty() {
            ev.kind.to_string()
        } else {
            "lsp".to_string()
        };
        let method = if !ev.method.is_empty() {
            ev.method.clone()
        } else if !ev.command.is_empty() {
            crate::metrics::command_method(&ev.command)
        } else {
            "unknown".to_string()
        };
        let engine = if !ev.engine.is_empty() {
            ev.engine.clone()
        } else {
            "none".to_string()
        };
        let error_class = if !ev.ok {
            ev.error_class.clone().unwrap_or_else(|| {
                crate::metrics::classify_error(&ev.command, ev.exit_code).to_string()
            })
        } else {
            String::new()
        };

        let key = OpKey {
            category,
            method,
            engine: engine.clone(),
            status,
            error_class: error_class.clone(),
        };

        let entry = agg_map.entry(key).or_default();
        entry.count += 1;
        entry.duration_ms += ev.duration_ms;
        entry.items += ev.items;
        entry.bytes += ev.bytes;

        if let Some(ref compiler) = ev.compiler {
            let ckey = CompKey {
                engine,
                compiler: compiler.clone(),
                status,
                error_class,
            };
            let c_entry = comp_map.entry(ckey).or_default();
            c_entry.count += 1;
            c_entry.duration_ms += ev.duration_ms;
        }
    });

    if !agg_map.is_empty() {
        let _ = writeln!(
            out,
            "# HELP prod_code_operations_total Total number of operations processed by the gateway.\n# TYPE prod_code_operations_total counter"
        );
        for (k, agg) in &agg_map {
            let cat = escape_label_value(&k.category);
            let meth = escape_label_value(&k.method);
            let eng = escape_label_value(&k.engine);
            if k.error_class.is_empty() {
                let _ = writeln!(
                    out,
                    "prod_code_operations_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\",status=\"{}\"}} {}",
                    k.status, agg.count
                );
            } else {
                let err = escape_label_value(&k.error_class);
                let _ = writeln!(
                    out,
                    "prod_code_operations_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\",status=\"{}\",error_class=\"{err}\"}} {}",
                    k.status, agg.count
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP prod_code_operation_duration_seconds_total Total duration of completed operations in seconds.\n# TYPE prod_code_operation_duration_seconds_total counter"
        );
        for (k, agg) in &agg_map {
            let cat = escape_label_value(&k.category);
            let meth = escape_label_value(&k.method);
            let eng = escape_label_value(&k.engine);
            let secs = agg.duration_ms as f64 / 1000.0;
            let _ = writeln!(
                out,
                "prod_code_operation_duration_seconds_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\",status=\"{}\"}} {:.6}",
                k.status, secs
            );
        }

        let _ = writeln!(
            out,
            "# HELP prod_code_operation_items_total Total items processed or returned by operations.\n# TYPE prod_code_operation_items_total counter"
        );
        for (k, agg) in &agg_map {
            if agg.items > 0 {
                let cat = escape_label_value(&k.category);
                let meth = escape_label_value(&k.method);
                let eng = escape_label_value(&k.engine);
                let _ = writeln!(
                    out,
                    "prod_code_operation_items_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\"}} {}",
                    agg.items
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP prod_code_operation_bytes_total Total bytes transferred or produced by operations.\n# TYPE prod_code_operation_bytes_total counter"
        );
        for (k, agg) in &agg_map {
            if agg.bytes > 0 {
                let cat = escape_label_value(&k.category);
                let meth = escape_label_value(&k.method);
                let eng = escape_label_value(&k.engine);
                let _ = writeln!(
                    out,
                    "prod_code_operation_bytes_total{{node=\"{node}\",category=\"{cat}\",method=\"{meth}\",engine=\"{eng}\"}} {}",
                    agg.bytes
                );
            }
        }
    }

    if !comp_map.is_empty() {
        let _ = writeln!(
            out,
            "# HELP prod_code_compilations_total Total number of compilations executed.\n# TYPE prod_code_compilations_total counter"
        );
        for (k, agg) in &comp_map {
            let eng = escape_label_value(&k.engine);
            let comp = escape_label_value(&k.compiler);
            let _ = writeln!(
                out,
                "prod_code_compilations_total{{node=\"{node}\",engine=\"{eng}\",compiler=\"{comp}\",status=\"{}\"}} {}",
                k.status, agg.count
            );
        }

        let _ = writeln!(
            out,
            "# HELP prod_code_compilation_duration_seconds_total Total duration of compilation runs in seconds.\n# TYPE prod_code_compilation_duration_seconds_total counter"
        );
        for (k, agg) in &comp_map {
            let eng = escape_label_value(&k.engine);
            let comp = escape_label_value(&k.compiler);
            let secs = agg.duration_ms as f64 / 1000.0;
            let _ = writeln!(
                out,
                "prod_code_compilation_duration_seconds_total{{node=\"{node}\",engine=\"{eng}\",compiler=\"{comp}\",status=\"{}\"}} {:.3}",
                k.status, secs
            );
        }

        if comp_map.iter().any(|(k, _)| k.status == "error") {
            let _ = writeln!(
                out,
                "# HELP prod_code_compilation_errors_total Total number of compilation failures by error class.\n# TYPE prod_code_compilation_errors_total counter"
            );
            for (k, agg) in &comp_map {
                if k.status == "error" {
                    let eng = escape_label_value(&k.engine);
                    let comp = escape_label_value(&k.compiler);
                    let err = escape_label_value(if k.error_class.is_empty() {
                        "unknown"
                    } else {
                        &k.error_class
                    });
                    let _ = writeln!(
                        out,
                        "prod_code_compilation_errors_total{{node=\"{node}\",engine=\"{eng}\",compiler=\"{comp}\",error_class=\"{err}\"}} {}",
                        agg.count
                    );
                }
            }
        }
    }
}
