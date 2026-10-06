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
use crate::commands::refactor::*;
use anyhow::Result;

pub async fn dispatch_refactor_ast(cmd: Commands, cx: &DispatchContext<'_>) -> Result<()> {
    match cmd {
        Commands::Rename {
            file,
            line,
            col,
            new_name,
            accessors,
            comments,
            force,
        } => {
            run_rename(
                cx.remote, &file, line, col, &new_name, accessors, comments, force,
            )
            .await
        }
        Commands::SafeDelete { file, line, col } => {
            run_safe_delete(cx.remote, &file, line, col).await
        }
        Commands::EncapsulateField {
            symbol,
            line,
            character,
            path,
            field,
            class,
            by_value,
            verify,
            apply,
            force,
        } => {
            run_encapsulate_field_cli(
                cx.remote, symbol, line, character, path, field, class, by_value, verify, apply,
                force,
            )
            .await
        }
        Commands::ExtractParameter {
            file,
            line,
            character,
            to,
            name,
            ty,
            replace_all,
            verify,
            apply,
            force,
        } => {
            run_extract_parameter_cli(
                cx.remote,
                &file,
                line,
                character,
                &to,
                name,
                ty,
                replace_all,
                verify,
                apply,
                force,
            )
            .await
        }
        Commands::ParameterObject {
            symbol,
            params,
            name,
            binding,
            path,
            verify,
            apply,
            force,
        } => {
            run_parameter_object_cli(
                cx.remote, symbol, params, name, binding, path, verify, apply, force,
            )
            .await
        }
        Commands::Move {
            symbol,
            to,
            path,
            verify,
            apply,
            force,
        } => run_move_cli(cx.remote, symbol, to, path, verify, apply, force).await,
        Commands::MoveMethod {
            file,
            line,
            col,
            to_param,
            to_type,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "line": line,
                "character": col,
                "apply": apply,
                "force": force,
            });
            if let Some(to_param) = to_param {
                args["to_param"] = serde_json::Value::String(to_param);
            }
            if let Some(to_type) = to_type {
                args["to_type"] = serde_json::Value::String(to_type);
            }
            run_tool(cx.remote, "code_move_method", args).await
        }
        Commands::MoveModule {
            file,
            to,
            verify,
            apply,
            force,
        } => {
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let to = if to.is_absolute() {
                to
            } else {
                std::env::current_dir()?.join(to)
            };
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "to": to.to_string_lossy(),
                "apply": apply,
                "force": force,
            });
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(cx.remote, "code_move_module", args).await
        }
        Commands::ChangeSignature {
            symbol,
            params,
            remove_all: _,
            returns,
            visibility,
            asyncness,
            path,
            verify,
            apply,
            force,
        } => {
            run_change_signature_cli(
                cx.remote, symbol, params, returns, visibility, asyncness, path, verify, apply,
                force,
            )
            .await
        }
        Commands::SchemaRename {
            field,
            to,
            path,
            repos,
            verify,
            apply,
            force,
            workspace_edit,
        } => {
            run_schema_rename_cli(
                cx.remote,
                field,
                to,
                path,
                repos,
                verify,
                apply,
                force,
                workspace_edit,
            )
            .await
        }
        Commands::MigrateType {
            symbol,
            to,
            line,
            character,
            path,
            convert,
            transitive,
            apply,
            force,
        } => {
            run_migrate_type_cli(
                cx.remote, symbol, to, line, character, path, convert, transitive, apply, force,
            )
            .await
        }
        Commands::Assist {
            file,
            line,
            col,
            id,
            to,
            subtype,
        } => {
            run_assist(
                cx.remote,
                &file,
                line,
                col,
                to.as_deref(),
                Some(&id),
                subtype,
            )
            .await
        }
        Commands::Assists {
            file,
            line,
            col,
            to,
        } => run_assist(cx.remote, &file, line, col, to.as_deref(), None, None).await,
        Commands::Codemod { rule, path, apply } => {
            run_codemod_cli(cx.remote, rule, path, apply).await
        }
        Commands::Fixture {
            symbol,
            depth,
            no_verify,
            path,
            builder,
            builder_name,
            randomized,
            mock,
            language,
        } => {
            run_fixture_cli(
                cx.remote,
                symbol,
                depth,
                !no_verify,
                path,
                builder,
                builder_name,
                randomized,
                mock,
                language,
            )
            .await
        }
        _ => unreachable!(),
    }
}
