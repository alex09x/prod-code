/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Intent search from the client side (roadmap 8.4): ask the gateway to rank the workspace's
//! declarations against a question, and render the hits. Shared by the MCP tool `code_search`
//! and the CLI `prod-code search`.

use crate::sync::{WorkspaceIdentity, push_workspace_sync, workspace_identity};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, SearchRequest, SearchResponse, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use tokio_util::codec::Framed;

/// Sends the query and returns the gateway's answer.
pub async fn search(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    limit: usize,
    subpath: Option<&str>,
) -> Result<SearchResponse> {
    anyhow::ensure!(!query.trim().is_empty(), "empty query");
    let identity: WorkspaceIdentity = workspace_identity(root);
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    push_workspace_sync(&mut framed, root, &identity, None)
        .await
        .context("pre-flight workspace sync failed")?;
    framed
        .send(WireMessage::SearchRequest(SearchRequest {
            client_workspace_root: root.to_string_lossy().to_string(),
            base_workspace_name: Some(identity.name.clone()),
            query: query.to_string(),
            limit,
            subpath: subpath.map(str::to_string),
            client_agent: Some(prod_code_protocol::detect_client_agent()),
            client_host: Some(prod_code_protocol::client_host()),
        }))
        .await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let msg = tokio::time::timeout_at(deadline, framed.next())
            .await
            .map_err(|_| anyhow::anyhow!("gateway search timed out after 30s"))?;
        match msg {
            Some(Ok(WireMessage::SearchResponse(resp))) => {
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "search finished".to_string(),
                    })
                    .await;
                if let Some(error) = resp.error {
                    anyhow::bail!("search refused: {error}");
                }
                return Ok(resp);
            }
            Some(Ok(WireMessage::Pong)) | Some(Ok(WireMessage::LspPayload(_))) => {}
            Some(Ok(other)) => anyhow::bail!("unexpected message during search: {other:?}"),
            Some(Err(e)) => anyhow::bail!("frame decode error during search: {e}"),
            None => anyhow::bail!("gateway closed the connection during the search"),
        }
    }
}

/// How the ranking was made: by words only, or by words and meaning, and how much of the
/// index has its vectors yet, plus typed graph fusion when applied.
fn ranking(resp: &SearchResponse) -> String {
    let graph_note = if resp.graph_fused.unwrap_or(false) {
        " and typed graph"
    } else {
        ""
    };
    match resp.dense {
        None => format!(
            "lexical{graph_note} only: the gateway has no embedding model, so a question sharing no words with the code or its comments finds nothing"
        ),
        Some(d) if d.embedded >= resp.indexed_declarations => {
            format!("ranked by words{graph_note} and by meaning")
        }
        Some(d) => format!(
            "ranked by words{graph_note}{}; {} of {} declarations embedded so far, the rest in the background",
            if d.used { " and by meaning" } else { " only" },
            d.embedded,
            resp.indexed_declarations
        ),
    }
}

/// One line per hit, best first, with the doc sentence that earned it and attributable rank reasons.
pub fn render(resp: &SearchResponse, query: &str) -> String {
    if resp.hits.is_empty() {
        return format!(
            "no declaration matches `{query}` ({} declarations in {} files searched; {})",
            resp.indexed_declarations,
            resp.indexed_files,
            ranking(resp)
        );
    }
    let mut out = format!(
        "{} hit(s) for `{}` in {} ms ({} declarations, {} files; {})\n",
        resp.hits.len(),
        query,
        resp.took_ms,
        resp.indexed_declarations,
        resp.indexed_files,
        ranking(resp)
    );
    for (i, hit) in resp.hits.iter().enumerate() {
        let container = hit
            .container
            .as_deref()
            .map(|c| format!("{c}::"))
            .unwrap_or_default();
        out.push_str(&format!(
            "\n{:>2}. [{}] {}{}  {}:{}\n    {}\n",
            i + 1,
            hit.kind,
            container,
            hit.name,
            hit.file,
            hit.line,
            hit.signature
        ));
        if !hit.doc.is_empty() {
            out.push_str(&format!("    {}\n", hit.doc));
        }
        if let Some(reasons) = &hit.rank_reasons
            && !reasons.is_empty()
        {
            let score_str = hit
                .score
                .as_deref()
                .map(|s| format!(" [score {s}]"))
                .unwrap_or_default();
            out.push_str(&format!(
                "    attribution{score_str}: {}\n",
                reasons.join("; ")
            ));
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prod_code_protocol::SearchHit;

    fn resp(hits: Vec<SearchHit>) -> SearchResponse {
        SearchResponse {
            server_workspace_root: "/srv/ws".to_string(),
            hits,
            indexed_files: 120,
            indexed_declarations: 2400,
            took_ms: 7,
            error: None,
            dense: None,
            graph_fused: None,
        }
    }

    #[test]
    fn renders_hits_best_first_with_their_doc() {
        let text = render(
            &resp(vec![SearchHit {
                file: "src/place.rs".into(),
                line: 42,
                kind: "function".into(),
                name: "place".into(),
                container: Some("ServerState".into()),
                signature: "pub fn place(&self, name: &str) -> Option<Node>".into(),
                doc: "Decides which node runs a workspace.".into(),
                score: None,
                rank_reasons: None,
            }]),
            "which node runs a workspace",
        );
        assert!(
            text.contains("1. [function] ServerState::place  src/place.rs:42"),
            "{text}"
        );
        assert!(
            text.contains("Decides which node runs a workspace."),
            "{text}"
        );
        assert!(text.contains("2400 declarations, 120 files"), "{text}");
    }

    #[test]
    fn renders_hits_with_attribution_and_graph_fusion() {
        let mut r = resp(vec![SearchHit {
            file: "src/search.rs".into(),
            line: 112,
            kind: "struct".into(),
            name: "WorkspaceIndex".into(),
            container: None,
            signature: "pub struct WorkspaceIndex".into(),
            doc: "Primary workspace declaration index.".into(),
            score: Some("0.0385".into()),
            rank_reasons: Some(vec![
                "lexical: matched 'workspace'".into(),
                "graph: struct in-degree 12 (centrality 1.76)".into(),
            ]),
        }]);
        r.graph_fused = Some(true);
        let text = render(&r, "workspace index");
        assert!(text.contains("lexical and typed graph only"), "{text}");
        assert!(text.contains("attribution [score 0.0385]: lexical: matched 'workspace'; graph: struct in-degree 12 (centrality 1.76)"), "{text}");
    }

    #[test]
    fn says_why_an_empty_result_is_empty() {
        let text = render(&resp(Vec::new()), "kubernetes ingress");
        assert!(
            text.contains("no declaration matches `kubernetes ingress`"),
            "{text}"
        );
        assert!(text.contains("lexical only"), "{text}");
        let mut partial = resp(Vec::new());
        partial.dense = Some(prod_code_protocol::DenseStatus {
            used: false,
            embedded: 0,
        });
        let text = render(&partial, "kubernetes ingress");
        assert!(
            text.contains("ranked by words only; 0 of 2400 declarations embedded so far"),
            "{text}"
        );
        partial.dense = Some(prod_code_protocol::DenseStatus {
            used: true,
            embedded: 2400,
        });
        assert!(render(&partial, "q").contains("ranked by words and by meaning)"));
        partial.dense = Some(prod_code_protocol::DenseStatus {
            used: true,
            embedded: 1200,
        });
        assert!(render(&partial, "q").contains("by meaning; 1200 of 2400"));
    }

    #[test]
    fn renders_a_hit_without_a_container_or_a_doc() {
        let text = render(
            &resp(vec![SearchHit {
                file: "src/free.rs".into(),
                line: 3,
                kind: "function".into(),
                name: "free_fn".into(),
                container: None,
                signature: "pub fn free_fn()".into(),
                doc: String::new(),
                score: None,
                rank_reasons: None,
            }]),
            "free function",
        );
        // No `Container::` prefix, and no doc line under the signature.
        assert!(
            text.contains(" 1. [function] free_fn  src/free.rs:3\n"),
            "{text}"
        );
        assert!(text.trim_end().ends_with("pub fn free_fn()"), "{text}");
    }

    #[tokio::test]
    async fn refuses_an_empty_query_without_reaching_the_network() {
        let unreachable: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let root = std::env::temp_dir();
        let err = search(unreachable, &root, "   ", 0, None)
            .await
            .expect_err("an empty query is refused");
        assert!(format!("{err:#}").contains("empty query"));
    }
}
