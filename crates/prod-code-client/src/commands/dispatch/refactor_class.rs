/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::DispatchContext;
use crate::cli::Commands;
use crate::commands::common::run_tool;
use anyhow::Result;
use std::path::Path;

pub async fn dispatch_refactor_class(cmd: Commands, cx: &DispatchContext<'_>) -> Result<()> {
    match cmd {
        Commands::Generify {
            symbol,
            param,
            bound,
            type_param,
            line,
            character,
            path,
            function,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "param": param,
                "bound": bound,
                "type_param": type_param,
                "apply": apply,
                "force": force,
            });
            if let Some(p) = path {
                args["path"] = serde_json::Value::String(p);
                args["symbol"] = serde_json::Value::String(function.unwrap_or(symbol));
            } else if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
                if let Some(f) = function {
                    args["symbol"] = serde_json::Value::String(f);
                }
            } else {
                let p = Path::new(&symbol);
                if p.extension().is_some() {
                    args["path"] = serde_json::Value::String(symbol);
                    if let Some(f) = function {
                        args["symbol"] = serde_json::Value::String(f);
                    }
                } else {
                    args["symbol"] = serde_json::Value::String(symbol);
                }
            }
            run_tool(cx.remote, "code_generify", args).await
        }
        Commands::InvertBoolean {
            symbol,
            new_name,
            line,
            character,
            path,
            function,
            verify,
            apply,
            force,
        } => {
            let mut args =
                serde_json::json!({ "new_name": new_name, "apply": apply, "force": force });
            let is_file_path = std::path::Path::new(&symbol).extension().is_some()
                || std::path::Path::new(&symbol).exists();
            if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
            } else if is_file_path {
                args["path"] = serde_json::Value::String(symbol);
            } else {
                args["symbol"] = serde_json::Value::String(symbol);
            }
            if let Some(path) = path {
                args["path"] = serde_json::Value::String(path);
            }
            if let Some(function) = function {
                args["function"] = serde_json::Value::String(function);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(cx.remote, "code_invert_boolean", args).await
        }
        Commands::ConvertToMethod {
            symbol,
            line,
            character,
            path,
            method,
            class,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({ "apply": apply, "force": force });
            let is_file_path = std::path::Path::new(&symbol).extension().is_some()
                || std::path::Path::new(&symbol).exists();
            if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
            } else if is_file_path {
                args["path"] = serde_json::Value::String(symbol);
            } else if method.is_some() {
                if path.is_some() {
                    args["class_name"] = serde_json::Value::String(symbol);
                } else {
                    args["path"] = serde_json::Value::String(symbol);
                }
            } else {
                args["symbol"] = serde_json::Value::String(symbol);
            }
            if let Some(path) = path {
                args["path"] = serde_json::Value::String(path);
            }
            if let Some(method) = method {
                args["method"] = serde_json::Value::String(method);
            }
            if let Some(class) = class {
                args["class_name"] = serde_json::Value::String(class);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(cx.remote, "code_convert_to_method", args).await
        }
        Commands::InlineParameter {
            file,
            line,
            col,
            function,
            param,
            verify,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                args["line"] = serde_json::json!(l);
            }
            if let Some(c) = col {
                args["character"] = serde_json::json!(c);
            }
            if let Some(f) = function {
                args["function"] = serde_json::Value::String(f);
            }
            if let Some(p) = param {
                args["parameter"] = serde_json::Value::String(p);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            run_tool(cx.remote, "code_inline_parameter", args).await
        }
        Commands::ExtractDelegate {
            file,
            line,
            col,
            symbol,
            fields,
            methods,
            name,
            field,
            verify,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "fields": fields,
                "methods": methods,
                "name": name,
                "field": field,
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                args["line"] = serde_json::Value::Number(l.into());
            }
            if let Some(c) = col {
                args["character"] = serde_json::Value::Number(c.into());
            }
            if let Some(s) = symbol {
                args["symbol"] = serde_json::Value::String(s);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            run_tool(cx.remote, "code_extract_delegate", args).await
        }
        Commands::ExtractTrait {
            file,
            line,
            col,
            methods,
            name,
            no_migrate_callers,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            run_tool(
                cx.remote,
                "code_extract_trait",
                serde_json::json!({
                    "path": abs.to_string_lossy(),
                    "line": line,
                    "character": col,
                    "methods": methods,
                    "name": name,
                    "migrate_callers": !no_migrate_callers,
                    "apply": apply,
                    "force": force,
                }),
            )
            .await
        }
        Commands::LoopToIterator {
            file,
            line,
            col,
            symbol,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut payload = serde_json::json!({
                "path": abs.to_string_lossy(),
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                payload["line"] = serde_json::json!(l);
            }
            if let Some(c) = col {
                payload["character"] = serde_json::json!(c);
            }
            if let Some(s) = symbol {
                payload["symbol"] = serde_json::json!(s);
            }
            run_tool(cx.remote, "code_loop_to_iterator", payload).await
        }
        Commands::MakeStatic {
            symbol,
            line,
            character,
            path,
            method,
            class,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({ "apply": apply, "force": force });
            let is_file_path = std::path::Path::new(&symbol).extension().is_some()
                || std::path::Path::new(&symbol).exists();
            if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
            } else if is_file_path {
                args["path"] = serde_json::Value::String(symbol);
            } else if method.is_some() {
                if path.is_some() {
                    args["class_name"] = serde_json::Value::String(symbol);
                } else {
                    args["path"] = serde_json::Value::String(symbol);
                }
            } else {
                args["symbol"] = serde_json::Value::String(symbol);
            }
            if let Some(path) = path {
                args["path"] = serde_json::Value::String(path);
            }
            if let Some(method) = method {
                args["method"] = serde_json::Value::String(method);
            }
            if let Some(class) = class {
                args["class_name"] = serde_json::Value::String(class);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(cx.remote, "code_make_static", args).await
        }
        Commands::WrapReturn {
            symbol,
            wrapper,
            constructor,
            error,
            path,
            function,
            line,
            character,
            verify,
            apply,
            force,
        } => {
            let mut args =
                serde_json::json!({ "wrapper": wrapper, "apply": apply, "force": force });
            if let Some(c) = constructor {
                args["constructor"] = serde_json::Value::String(c);
            }
            if let Some(p) = path {
                args["path"] = serde_json::Value::String(p);
                args["symbol"] = serde_json::Value::String(function.unwrap_or(symbol));
            } else if let Some(line) = line {
                args["path"] = serde_json::Value::String(symbol);
                args["line"] = serde_json::Value::from(line);
                args["character"] = serde_json::Value::from(character);
                if let Some(f) = function {
                    args["symbol"] = serde_json::Value::String(f);
                }
            } else {
                let p = Path::new(&symbol);
                if p.extension().is_some() {
                    args["path"] = serde_json::Value::String(symbol);
                    if let Some(f) = function {
                        args["symbol"] = serde_json::Value::String(f);
                    }
                } else {
                    args["symbol"] = serde_json::Value::String(symbol);
                }
            }
            if let Some(error) = error {
                args["error"] = serde_json::Value::String(error);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(cx.remote, "code_wrap_return", args).await
        }
        _ => unreachable!(),
    }
}
