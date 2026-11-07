/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, bail};
use std::net::SocketAddr;

/// The nodes to ask, the address being forgotten last: it is the one that is not answering.
fn ask_order(nodes: &[SocketAddr], address: &str) -> Vec<SocketAddr> {
    let mut order = nodes.to_vec();
    order.sort_by_key(|node| node.to_string() == address);
    order
}

/// Asks the first node that answers to forget `address`; it tells the others. What this machine
/// remembers of the address (the node cache and the placements) is dropped first, so that the
/// next `prod-code cluster` does not read it back.
pub async fn run_forget_peer(nodes: &[SocketAddr], address: &str) -> Result<()> {
    prod_code_mcp::cluster::forget_local(address);
    let mut last_error = None;
    for node in ask_order(nodes, address) {
        match prod_code_mcp::cluster::forget_peer(node, address).await {
            Ok(answer) => {
                println!(
                    "{node}: {address} was {}",
                    if answer.forgotten {
                        "listed and is forgotten"
                    } else {
                        "not listed there; it will not be learned again"
                    }
                );
                for other in &answer.forwarded {
                    println!("  also forgotten by {other}");
                }
                for other in &answer.failed {
                    println!(
                        "  could not tell {other}: run `prod-code forget-peer {address}` again once it is up, or against it"
                    );
                }
                return Ok(());
            }
            Err(error) => last_error = Some(format!("{node}: {error}")),
        }
    }
    match last_error {
        Some(error) => bail!("no node could be asked to forget {address}: {error}"),
        None => bail!("no node is configured; set PROD_CODE_REMOTE or pass --remote"),
    }
}

#[cfg(test)]
mod tests {
    use super::ask_order;
    use std::net::SocketAddr;

    #[test]
    fn the_forgotten_address_is_asked_last() {
        let a: SocketAddr = "192.0.2.1:9400".parse().unwrap();
        let dead: SocketAddr = "192.0.2.40:9400".parse().unwrap();
        let b: SocketAddr = "192.0.2.2:9400".parse().unwrap();
        assert_eq!(ask_order(&[dead, a, b], "192.0.2.40:9400"), [a, b, dead]);
    }
}
