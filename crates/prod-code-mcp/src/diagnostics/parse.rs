/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::diagnostics::types::{
    DiagnosticsReport, DocDiagnostic, HallucinationInterception, HallucinationKind,
};

pub const INVALID_DIAGNOSTICS: &str = "prod-code-invalid-diagnostics";

/// A missing or malformed required report is unavailable evidence, not a clean file. Keep
/// this as a diagnostic so every CLI/MCP consumer preserves its unsuccessful status.
pub fn invalid_report(file: &str, reason: &str) -> DiagnosticsReport {
    DiagnosticsReport {
        file: file.to_string(),
        errors: 1,
        warnings: 0,
        items: vec![DocDiagnostic {
            severity: "error".to_string(),
            code: Some(INVALID_DIAGNOSTICS.to_string()),
            message: format!(
                "the analyzer returned invalid diagnostics: {reason}; the file was not validated. Retry the language server or use an explicit compiler check"
            ),
            line: 1,
            col: 1,
            source: Some("prod-code".to_string()),
            note: None,
            end: None,
        }],
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    }
}

pub fn diagnostic_position(value: Option<&serde_json::Value>) -> Result<(u32, u32), String> {
    let value = value.ok_or("missing range endpoint")?;
    let coordinate = |key| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| format!("invalid or unrepresentable {key} coordinate"))
    };
    Ok((coordinate("line")?, coordinate("character")?))
}

pub fn parse_diagnostic(d: &serde_json::Value) -> Result<DocDiagnostic, String> {
    let range = d.get("range").ok_or("missing diagnostic range")?;
    let start = diagnostic_position(range.get("start"))?;
    let end = diagnostic_position(range.get("end"))?;
    if end < start {
        return Err("diagnostic range ends before it starts".into());
    }
    let severity = match d.get("severity") {
        None => "error",
        Some(v) => match v.as_u64() {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "info",
            Some(4) => "hint",
            _ => return Err("invalid diagnostic severity".into()),
        },
    };
    let code = match d.get("code") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(n) if n.as_i64().is_some() => Some(n.to_string()),
        Some(_) => return Err("diagnostic code is neither a string nor an integer".into()),
    };
    let source = match d.get("source") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("diagnostic source is not a string".into()),
    };
    let message = d
        .get("message")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing diagnostic message")?
        .to_string();
    Ok(DocDiagnostic {
        severity: severity.into(),
        code,
        message,
        line: start.0,
        col: start.1,
        source,
        note: None,
        end: Some(end),
    })
}

pub fn is_borrow_checker_error_code(code: &str) -> bool {
    let clean = code.trim().trim_start_matches('[').trim_end_matches(']');
    matches!(
        clean,
        "E0382"
            | "E0499"
            | "E0502"
            | "E0503"
            | "E0505"
            | "E0506"
            | "E0507"
            | "E0515"
            | "E0521"
            | "E0596"
            | "E0597"
            | "E0716"
    )
}

pub fn extract_method_name(msg: &str) -> Option<String> {
    if let Some(start) = msg.find("no method named `") {
        let rest = &msg[start + "no method named `".len()..];
        if let Some(end) = rest.find('`') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(start) = msg.find("Property '") {
        let rest = &msg[start + "Property '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(idx) = msg.find(" undefined") {
        let prefix = &msg[..idx];
        if let Some(dot) = prefix.rfind('.') {
            return Some(prefix[dot + 1..].trim().to_string());
        }
    }
    if let Some(start) = msg.find("Cannot access member '") {
        let rest = &msg[start + "Cannot access member '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(start) = msg.find("no member named '") {
        let rest = &msg[start + "no member named '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(start) = msg.find("has no attribute '") {
        let rest = &msg[start + "has no attribute '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

pub fn classify_hallucination(d: &DocDiagnostic) -> Option<HallucinationInterception> {
    let msg_lower = d.message.to_ascii_lowercase();
    let code_str = d.code.as_deref().unwrap_or("");

    if is_borrow_checker_error_code(code_str)
        || msg_lower.contains("borrow")
        || msg_lower.contains("use of moved value")
        || msg_lower.contains("does not live long enough")
        || msg_lower.contains("returns a value referencing data owned by the current function")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::BorrowCheckerError,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Review value ownership, adjust borrow scopes, clone, or adjust reference lifetimes".to_string()),
        });
    }

    if code_str == "unresolved-method"
        || code_str == "no-such-field"
        || code_str == "2339"
        || code_str == "2551"
        || msg_lower.contains("no method named")
        || msg_lower.contains("does not exist on type")
        || (msg_lower.contains("has no field or method") && msg_lower.contains("undefined"))
        || msg_lower.contains("cannot access member")
        || msg_lower.contains("no member named")
        || msg_lower.contains("has no attribute")
    {
        let target = extract_method_name(&d.message);
        return Some(HallucinationInterception {
            kind: HallucinationKind::InvalidMethodInvocation,
            symbol_or_target: target,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Verify method existence on type or check trait imports".to_string()),
        });
    }

    if code_str == "syntax-error"
        || code_str == "parse-error"
        || msg_lower.contains("syntax error")
        || (msg_lower.contains("expected ")
            && (msg_lower.contains("found `;`")
                || msg_lower.contains("found `}`")
                || msg_lower.contains("found `{`")
                || msg_lower.contains("expected token")))
        || msg_lower.contains("unclosed delimiter")
        || msg_lower.contains("unexpected token")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::SyntaxError,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Correct unbalanced syntax or missing delimiter".to_string()),
        });
    }

    if code_str == "E0308"
        || code_str == "E0061"
        || code_str == "type-mismatch"
        || code_str == "2345"
        || code_str == "2554"
        || msg_lower.contains("mismatched types")
        || (msg_lower.contains("expected ") && msg_lower.contains("found "))
        || msg_lower.contains("is not assignable to parameter")
        || (msg_lower.contains("this function takes") && msg_lower.contains("arguments but"))
        || (msg_lower.contains("cannot use") && msg_lower.contains("value in argument"))
        || msg_lower.contains("cannot be assigned to parameter")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::IncorrectArgumentType,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Verify function signature and argument types".to_string()),
        });
    }

    if code_str == "E0425"
        || code_str == "unresolved-ident"
        || code_str == "unresolved-path"
        || code_str == "2304"
        || msg_lower.contains("cannot find value")
        || msg_lower.contains("cannot find function")
        || msg_lower.contains("cannot find name")
        || msg_lower.contains("undefined:")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::UnresolvedIdentifier,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Check symbol spelling or add missing import".to_string()),
        });
    }

    None
}

pub fn parse_items(file: &str, result: &serde_json::Value) -> DiagnosticsReport {
    // This client never sends a previousResultId, so an unchanged report has no cached
    // evidence to refer to. Older adapters omit kind but still provide the complete items.
    if result
        .get("kind")
        .is_some_and(|kind| kind.as_str() != Some("full"))
    {
        return invalid_report(
            file,
            "expected a full report; no previous result was supplied",
        );
    }
    let Some(raw_items) = result
        .get("items")
        .and_then(serde_json::Value::as_array)
        .or_else(|| result.as_array())
    else {
        return invalid_report(file, "required items array is missing or malformed");
    };
    let mut items = Vec::with_capacity(raw_items.len());
    for (index, raw) in raw_items.iter().enumerate() {
        match parse_diagnostic(raw) {
            Ok(d) if d.code.as_deref() == Some("inactive-code") => {}
            Ok(d) => items.push(d),
            Err(reason) => return invalid_report(file, &format!("diagnostic {index}: {reason}")),
        }
    }
    let hallucinations = items
        .iter()
        .filter(|d| d.severity == "error")
        .filter_map(classify_hallucination)
        .collect();
    DiagnosticsReport {
        file: file.to_string(),
        errors: items.iter().filter(|d| d.severity == "error").count(),
        warnings: items.iter().filter(|d| d.severity == "warning").count(),
        items,
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations,
    }
}
