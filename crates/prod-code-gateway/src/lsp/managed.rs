/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) enum ManagedLsp<'a> {
    Go(&'a prod_code_engine_go::GoEngine),
    Generic(&'a prod_code_engine_generic::GenericLspEngine),
}

impl ManagedLsp<'_> {
    pub(crate) async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = match self {
            ManagedLsp::Go(engine) => engine.send_request(method, params).await?,
            ManagedLsp::Generic(engine) => engine.send_request(method, params).await?,
        };
        if let Some(err) = resp.get("error") {
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("request failed");
            anyhow::bail!("{method}: {message}");
        }
        Ok(resp
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// The diagnostics for the text last sent for `uri`. A server that answers a pull is asked
    /// for them (the native TypeScript server, and sourcekit-lsp, which does not advertise it);
    /// one that only publishes is waited for until it has published for that text, so a check
    /// of a proposed text is not answered with the errors of the text before it (#293), and a
    /// one-shot session still gets the first publication after its didOpen for quick fixes.
    /// When no publication for that text comes, this is an error, not an empty list (#471).
    pub(crate) async fn diagnostics_for(
        &self,
        uri: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        match self {
            ManagedLsp::Go(_) => Ok(Vec::new()),
            ManagedLsp::Generic(engine) => {
                if let Some(items) = engine.pull_diagnostics(uri).await {
                    return Ok(items);
                }
                Ok(engine
                    .current_diagnostics_for(uri, CURRENT_DIAGNOSTICS_WAIT)
                    .await?)
            }
        }
    }
}

/// How long an answer about a document's diagnostics waits for a publishing server to build the
/// text last sent. A C++ translation unit with heavy headers takes seconds on a cold server.
pub(crate) const CURRENT_DIAGNOSTICS_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
