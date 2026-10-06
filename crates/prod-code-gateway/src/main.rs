/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The gateway daemon: parse the command line, set logging up, and run the server.

use std::io::IsTerminal;
use anyhow::Result;
use clap::Parser;
use prod_code_gateway::ServerCli;

/// The gateway's own messages at `debug`, everything else at `info` — except the analyzer.
///
/// rust-analyzer's crates and salsa emit an `info` event for every query they execute. At
/// `info` a single diagnostics pass over a large file produced hundreds of thousands of lines,
/// the system journal dropped them by the hundred thousand every thirty seconds, and the
/// gateway's own lines were dropped with them (#95). `RUST_LOG` still overrides all of this.
const DEFAULT_LOG_FILTER: &str = "info,salsa=warn,\
base_db=warn,hir=warn,hir_def=warn,hir_expand=warn,hir_ty=warn,\
ide=warn,ide_db=warn,ide_diagnostics=warn,ide_assists=warn,ide_completion=warn,ide_ssr=warn,\
load_cargo=warn,project_model=warn,vfs=warn,syntax=warn,mbe=warn,\
ra_ap_base_db=warn,ra_ap_hir=warn,ra_ap_hir_def=warn,ra_ap_hir_expand=warn,ra_ap_hir_ty=warn,\
ra_ap_ide=warn,ra_ap_ide_db=warn,ra_ap_ide_diagnostics=warn,ra_ap_ide_assists=warn,ra_ap_ide_completion=warn,\
ra_ap_ide_ssr=warn,ra_ap_load_cargo=warn,ra_ap_project_model=warn,ra_ap_vfs=warn,\
prod_code_gateway=debug,prod_code_engine_rust=debug";

use prod_code_gateway::ensure_blocking_stdio;

fn main() -> Result<()> {
    ensure_blocking_stdio();
    // The exec shim is recognised before anything else: it sets up no logging, parses none of
    // the server's arguments and runs no async runtime, so that it stays small (#255).
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args
        .get(1)
        .is_some_and(|flag| flag == prod_code_gateway::exec_shim::SHIM_FLAG)
    {
        std::process::exit(prod_code_gateway::exec_shim::run(&args[2..]));
    }
    serve()
}

#[tokio::main]
async fn serve() -> Result<()> {
    ensure_blocking_stdio();
    tracing_subscriber::fmt()
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| DEFAULT_LOG_FILTER.into()),
        )
        .init();
    prod_code_gateway::run(ServerCli::parse()).await
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_LOG_FILTER;

    #[test]
    fn the_default_filter_parses_and_keeps_the_analyzer_quiet() {
        let filter: tracing_subscriber::EnvFilter = DEFAULT_LOG_FILTER.parse().expect("parses");
        let shown = filter.to_string();
        for directive in [
            "salsa=warn",
            "ra_ap_hir_ty=warn",
            "hir_ty=warn",
            "ide=warn",
            "prod_code_gateway=debug",
        ] {
            assert!(shown.contains(directive), "{directive} in {shown}");
        }
        assert!(
            !DEFAULT_LOG_FILTER.contains(' '),
            "no stray spaces from the line breaks"
        );
    }
}
