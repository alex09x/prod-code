/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ARTIFACT_LIST_LIMIT, ArtifactListing, PULL_CHUNK_BYTES, ProdCodeCodec, PullFileRequest,
    WireMessage,
};
use std::net::SocketAddr;
use std::path::{Component, Path};
use tokio_util::codec::Framed;

pub struct ArtifactListOutcome {
    pub node: SocketAddr,
    pub server_workspace_root: String,
    pub directory: String,
    pub listing: ArtifactListing,
}

impl ArtifactListOutcome {
    pub fn render(&self) -> String {
        let mut out = format!(
            "Artifacts in {} on node {}, copy {}:\n",
            self.directory, self.node, self.server_workspace_root
        );
        for entry in &self.listing.entries {
            out.push_str(&format!(
                "  {} {}\n",
                entry.kind,
                serde_json::to_string(&entry.name).unwrap()
            ));
        }
        if self.listing.truncated {
            out.push_str("Listing incomplete: entry limit reached or a name is not UTF-8.\n");
        }
        out
    }
}

/// List one remote directory without syncing, creating a workspace, or writing locally.
pub async fn list_remote_artifacts_from(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
) -> Result<ArtifactListOutcome> {
    let root = std::fs::canonicalize(root)?;
    let relative = if path.is_absolute() {
        path.strip_prefix(&root)?
    } else {
        path
    };
    anyhow::ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "list path must be inside the checkout: {}",
        path.display()
    );
    let name = relative
        .to_str()
        .context("list path must be UTF-8")?
        .to_string();
    let identity = super::workspace_identity(&root);
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| {
            format!(
                "cannot list artifacts on node {remote}; use the node printed by prod-code exec"
            )
        })?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::PullFileRequest(PullFileRequest {
            client_workspace_root: root.to_string_lossy().into_owned(),
            base_workspace_name: Some(identity.name),
            path: name.clone(),
            offset: 0,
            list_directory: true,
        }))
        .await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let response = loop {
        match tokio::time::timeout_at(deadline, framed.next())
            .await
            .context("artifact listing timed out")?
        {
            Some(Ok(WireMessage::PullFileResponse(response))) => break response,
            Some(Ok(WireMessage::Pong | WireMessage::LspPayload(_) | WireMessage::Auth(_))) => {
                continue;
            }
            _ => anyhow::bail!("artifact listing unavailable on node {remote}; update the gateway"),
        }
    };
    if let Some(error) = response.file.error {
        anyhow::bail!(
            "cannot list {name} on node {remote}, copy {}: {error}; older gateways do not support directory listing",
            response.server_workspace_root
        );
    }
    anyhow::ensure!(
        response.offset == 0
            && response.file.path == name
            && !response.file.truncated
            && response.file.is_executable.is_none(),
        "invalid artifact listing response; the gateway may be too old"
    );
    let bytes = response
        .file
        .content
        .context("artifact listing response had no content")?;
    anyhow::ensure!(
        bytes.len() as u64 == response.total_bytes && response.total_bytes <= PULL_CHUNK_BYTES,
        "invalid artifact listing size"
    );
    let listing: ArtifactListing = serde_json::from_slice(&bytes)
        .context("invalid artifact listing; the gateway may be too old")?;
    anyhow::ensure!(
        listing.entries.len() <= ARTIFACT_LIST_LIMIT,
        "artifact listing exceeds entry limit"
    );
    for entry in &listing.entries {
        let mut components = Path::new(&entry.name).components();
        anyhow::ensure!(
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
            "invalid artifact entry name"
        );
        anyhow::ensure!(
            matches!(
                entry.kind.as_str(),
                "file" | "directory" | "symlink" | "other"
            ),
            "invalid artifact entry kind"
        );
    }
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "listing finished".into(),
        })
        .await;
    Ok(ArtifactListOutcome {
        node: remote,
        server_workspace_root: response.server_workspace_root,
        directory: name,
        listing,
    })
}
