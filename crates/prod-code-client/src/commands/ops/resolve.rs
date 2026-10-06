/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::net::ToSocketAddrs;

#[derive(serde::Serialize)]
pub struct ResolveOutput {
    pub query: String,
    pub domain: String,
    pub is_internal: bool,
    pub project: Option<String>,
    pub designated_node: Option<String>,
    pub endpoints: Vec<String>,
    pub srv_records: Vec<prod_code_protocol::dns::DnsSrvRecord>,
    pub discovered_nodes: usize,
}

pub async fn run_resolve(domain: &str, json: bool) -> Result<()> {
    let clean_domain = domain.trim();
    let is_internal = prod_code_protocol::dns::is_code_internal_domain(clean_domain);
    let discovered = prod_code_mcp::cluster::discover_auto_nodes_sync();
    let project = prod_code_protocol::dns::extract_project_name(clean_domain);
    let designated_node = project.as_deref().and_then(|p| {
        prod_code_protocol::dns::resolve_project_node(p, &discovered).map(|n| n.addr.to_string())
    });
    let endpoints = if is_internal {
        prod_code_protocol::dns::resolve_smart_domain(clean_domain, &discovered, 9400)
            .unwrap_or_default()
    } else {
        match clean_domain.to_socket_addrs() {
            Ok(iter) => iter.collect(),
            Err(_) => match (clean_domain, 9400).to_socket_addrs() {
                Ok(iter) => iter.collect(),
                Err(_) => Vec::new(),
            },
        }
    };
    let srv_records = prod_code_protocol::dns::generate_srv_records(&discovered);

    if json {
        let out = ResolveOutput {
            query: domain.to_string(),
            domain: clean_domain.to_string(),
            is_internal,
            project,
            designated_node,
            endpoints: endpoints.iter().map(|e| e.to_string()).collect(),
            srv_records,
            discovered_nodes: discovered.len(),
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("⚡ prod-code resolve: {clean_domain}");
    println!("────────────────────────────────────────────────────");
    if is_internal {
        println!("Type:               Internal cluster domain (*.code.internal)");
        if let Some(ref proj) = project {
            if let Some(ref node) = designated_node {
                println!("Project:            {proj} -> node {node} (warm workspace / rendezvous)");
            } else {
                println!("Project:            {proj}");
            }
        }
    } else {
        println!("Type:               Standard host / external address");
    }
    println!("Discovered nodes:   {}", discovered.len());
    println!("Endpoints ({}):", endpoints.len());
    if endpoints.is_empty() {
        println!("  (none)");
    } else {
        for ep in &endpoints {
            println!("  - {ep}");
        }
    }
    if !srv_records.is_empty() {
        println!(
            "Dynamic SRV records ({}):",
            prod_code_protocol::dns::SRV_SERVICE_NAME
        );
        for srv in &srv_records {
            println!(
                "  - {} (priority: {}, weight: {}, port: {})",
                srv.target, srv.priority, srv.weight, srv.port
            );
        }
    }
    Ok(())
}
