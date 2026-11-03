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

    /// Pin this workspace to one gateway endpoint (`host:port`): every command, query, retry
    /// and failover stays on the addresses that name resolves to, or fails with the reason; a
    /// gateway too old to confirm the pin is refused before anything runs. `remote` pins to the
    /// `--remote` endpoint; `auto` turns off a `[cluster] pin` from `prod-code.toml`. Without a
    /// pin, `--remote` typed on the command line still limits work to the gateways it lists.
    #[arg(
        long,
        global = true,
        env = "PROD_CODE_PIN_NODE",
        value_name = "ENDPOINT"
    )]
    pub pin_node: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}
