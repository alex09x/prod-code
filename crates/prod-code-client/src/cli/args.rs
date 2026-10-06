/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::commands::Commands;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "prod-code",
    author = "Alex <alex@prod.codes>",
    version,
    about = "Remote Code Intelligence Client"
)]
pub struct Cli {
    /// Remote gateway address(es): `host:port[,host:port...]`. With several nodes the
    /// workspace is placed on one of them (rendezvous hashing, remembered locally, failover
    /// to the next alive node). Defaults to PROD_CODE_REMOTE or 127.0.0.1:9400.
    #[arg(
        short,
        long,
        global = true,
        env = "PROD_CODE_REMOTE",
        default_value = "127.0.0.1:9400"
    )]
    pub remote: String,

    #[command(subcommand)]
    pub command: Option<Commands>,
}
