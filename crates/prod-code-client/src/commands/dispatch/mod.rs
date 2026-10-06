/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod exec;
pub mod ops;
pub mod query;
pub mod refactor_ast;
pub mod refactor_class;
pub mod refactor_members;

pub use exec::*;
pub use ops::*;
pub use query::*;
pub use refactor_ast::*;
pub use refactor_class::*;
pub use refactor_members::*;

use crate::cli::Commands;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

pub struct DispatchContext<'a> {
    pub remote: SocketAddr,
    pub remotes: &'a [SocketAddr],
    pub placement_key: &'a str,
    pub cwd_engine: Option<&'static str>,
    pub lsp_engine: Option<&'static str>,
    pub cwd_root: Option<&'a Path>,
}

pub async fn dispatch(cmd: Commands, cx: DispatchContext<'_>) -> Result<()> {
    match cmd {
        Commands::Def { .. }
        | Commands::Hover { .. }
        | Commands::Refs { .. }
        | Commands::Callers { .. }
        | Commands::Callees { .. }
        | Commands::Impls { .. }
        | Commands::Supertypes { .. }
        | Commands::Symbols { .. }
        | Commands::Outline { .. }
        | Commands::Source { .. }
        | Commands::Search { .. }
        | Commands::Slice { .. }
        | Commands::Impact { .. }
        | Commands::DeadCode { .. }
        | Commands::Prune { .. }
        | Commands::Diagnostics { .. }
        | Commands::Diagnose { .. }
        | Commands::Dependencies { .. }
        | Commands::Duplicates { .. }
        | Commands::StructuralSearch { .. } => dispatch_query(cmd, &cx).await,

        Commands::Check { .. }
        | Commands::Lint { .. }
        | Commands::Test { .. }
        | Commands::Benchmarks { .. }
        | Commands::Exec { .. }
        | Commands::ShadowRun { .. }
        | Commands::DivergentBench { .. }
        | Commands::Bench { .. }
        | Commands::Validate { .. } => dispatch_exec(cmd, &cx).await,

        Commands::Lsp { .. }
        | Commands::Status { .. }
        | Commands::Cluster { .. }
        | Commands::Resolve { .. }
        | Commands::Metrics { .. }
        | Commands::ReportIssue { .. }
        | Commands::Mcp
        | Commands::Sync { .. }
        | Commands::Pull { .. }
        | Commands::ProposeExpression { .. }
        | Commands::Update { .. }
        | Commands::Package { .. }
        | Commands::Cert { .. } => dispatch_ops(cmd, &cx).await,

        Commands::Rename { .. }
        | Commands::SafeDelete { .. }
        | Commands::EncapsulateField { .. }
        | Commands::ExtractParameter { .. }
        | Commands::ParameterObject { .. }
        | Commands::Move { .. }
        | Commands::MoveMethod { .. }
        | Commands::MoveModule { .. }
        | Commands::ChangeSignature { .. }
        | Commands::SchemaRename { .. }
        | Commands::MigrateType { .. }
        | Commands::Assist { .. }
        | Commands::Assists { .. }
        | Commands::Codemod { .. }
        | Commands::Fixture { .. } => dispatch_refactor_ast(cmd, &cx).await,

        Commands::Generify { .. }
        | Commands::InvertBoolean { .. }
        | Commands::ConvertToMethod { .. }
        | Commands::InlineParameter { .. }
        | Commands::ExtractDelegate { .. }
        | Commands::ExtractTrait { .. }
        | Commands::LoopToIterator { .. }
        | Commands::MakeStatic { .. }
        | Commands::WrapReturn { .. } => dispatch_refactor_class(cmd, &cx).await,

        Commands::ExtractFunction { .. }
        | Commands::IntroduceVariable { .. }
        | Commands::ReplaceConstructorWithFactory { .. }
        | Commands::ReplaceConstructorWithBuilder { .. }
        | Commands::PullUp { .. }
        | Commands::PushDown { .. }
        | Commands::ReplaceInheritanceWithDelegation { .. }
        | Commands::ReplaceConditionalWithPolymorphism { .. }
        | Commands::ExtractInterface { .. }
        | Commands::ExtractField { .. } => dispatch_refactor_members(cmd, &cx).await,
    }
}
