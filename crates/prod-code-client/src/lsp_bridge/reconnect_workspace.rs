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
use futures_util::{SinkExt, stream::SplitSink};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use tokio_util::codec::Framed;

use super::state::LspStateTracker;

pub async fn replay_lsp_workspace_state(
    socket_tx: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    tracker: &LspStateTracker,
    files: &prod_code_client::editor_files::RemoteFiles,
) -> Result<()> {
    for config in &tracker.configuration_notifications {
        socket_tx
            .send(WireMessage::LspPayload(files.to_node(config)))
            .await
            .context("failed to send replayed configuration notification")?;
    }
    for doc in tracker.open_documents.values() {
        let did_open = serde_json::json!({"jsonrpc":"2.0", "method":"textDocument/didOpen",
            "params":{"textDocument":{"uri":doc.uri,"languageId":doc.language_id,
            "version":doc.version,"text":doc.text}}});
        socket_tx
            .send(WireMessage::LspPayload(
                files.to_node(&did_open.to_string()),
            ))
            .await
            .context("failed to send replayed didOpen notification")?;
    }
    Ok(())
}
