/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A save reaches the language server only after the saved file has (#1517). A
//! `textDocument/didSave` whose sync failed is held, not forwarded: the server would check the
//! node's old copy on save, and nothing would check again once a later sync brought the new
//! text. Held saves go out, in order, right after the next sync that succeeds.

/// How many failed saves are held at most; older ones are dropped first.
const MAX_HELD: usize = 64;
/// How often other editor traffic retries the sync for held saves.
const RETRY: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Default)]
pub struct SaveGate {
    held: Vec<String>,
    /// No retry for held saves before this, so a sync that keeps failing does not slow every
    /// keystroke.
    retry_at: Option<std::time::Instant>,
}

impl SaveGate {
    /// Whether the bridge must sync before forwarding this message: a save or a watched-file
    /// change always, anything else while a failed save waits for its contents, at most once
    /// per [`RETRY`].
    pub fn needs_sync(&self, saves_or_watches: bool) -> bool {
        saves_or_watches
            || !self.held.is_empty()
                && self
                    .retry_at
                    .is_none_or(|at| std::time::Instant::now() >= at)
    }

    /// The messages to forward now, in order, after a sync attempt for `message`: held saves
    /// and the message when the sync succeeded; nothing but the other messages when it failed,
    /// and a failed save is held.
    pub fn after_sync(&mut self, synced: bool, is_save: bool, message: String) -> Vec<String> {
        if synced {
            self.retry_at = None;
            let mut out = std::mem::take(&mut self.held);
            out.push(message);
            return out;
        }
        self.retry_at = Some(std::time::Instant::now() + RETRY);
        if is_save {
            if self.held.len() == MAX_HELD {
                self.held.remove(0);
            }
            self.held.push(message);
            return Vec::new();
        }
        vec![message]
    }

    pub async fn sync_and_flush(
        &mut self,
        client_method: Option<&str>,
        json_payload: &str,
        remote: std::net::SocketAddr,
        cwd: &std::path::Path,
        pushing: &tokio::sync::Mutex<()>,
        editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
        trace: &super::transport::LspTrace,
        files: &prod_code_client::editor_files::RemoteFiles,
        socket_tx: &mut futures_util::stream::SplitSink<
            tokio_util::codec::Framed<
                prod_code_protocol::AnyStream,
                prod_code_protocol::ProdCodeCodec,
            >,
            prod_code_protocol::WireMessage,
        >,
    ) -> bool {
        use futures_util::SinkExt;
        let is_save = client_method == Some("textDocument/didSave");
        let saves_or_watches = is_save || client_method == Some("workspace/didChangeWatchedFiles");
        if !self.needs_sync(saves_or_watches) {
            return true;
        }
        let generation = prod_code_mcp::watch::current_generation(cwd);
        let pushed = {
            let _one_at_a_time = pushing.lock().await;
            super::sync::push_checkout(remote, cwd).await
        };
        let synced = match pushed {
            Ok(()) => {
                prod_code_mcp::watch::mark_synced(cwd, generation);
                true
            }
            Err(err) => {
                tracing::debug!(error = %format!("{err:#}"), "sync before a save failed");
                if saves_or_watches {
                    let warning = prod_code_client::editor_files::push_failed_warning(&err);
                    super::transport::trace_message(trace, "<-", &warning);
                    let mut stdout = editor_out.lock().await;
                    let _ =
                        prod_code_client::editor_files::write_frame(&mut *stdout, &warning).await;
                }
                false
            }
        };
        let mut outgoing = self.after_sync(synced, is_save, json_payload.to_string());
        if outgoing.pop().is_none() {
            return false;
        }
        for held in outgoing {
            super::transport::trace_message(trace, "->", &held);
            if socket_tx
                .send(prod_code_protocol::WireMessage::LspPayload(
                    files.to_node(&held),
                ))
                .await
                .is_err()
            {
                break;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::SaveGate;

    /// A save whose sync failed waits; it goes out first after the next good sync (#1517).
    #[test]
    fn a_failed_save_waits_for_the_next_successful_sync() {
        let mut gate = SaveGate::default();
        assert!(!gate.needs_sync(false));
        assert!(gate.after_sync(false, true, "save-1".into()).is_empty());
        assert!(gate.needs_sync(true), "a save always syncs");
        assert!(
            !gate.needs_sync(false),
            "other traffic waits a retry interval"
        );
        gate.retry_at = Some(std::time::Instant::now());
        assert!(gate.needs_sync(false), "a held save asks for a sync");
        assert_eq!(
            gate.after_sync(false, false, "change".into()),
            ["change"],
            "other traffic keeps flowing"
        );
        assert_eq!(
            gate.after_sync(true, false, "hover".into()),
            ["save-1", "hover"]
        );
        assert!(!gate.needs_sync(false));
        assert_eq!(gate.after_sync(true, true, "save-2".into()), ["save-2"]);
    }

    #[test]
    fn held_saves_are_bounded() {
        let mut gate = SaveGate::default();
        for n in 0..70 {
            gate.after_sync(false, true, format!("save-{n}"));
        }
        let out = gate.after_sync(true, false, "x".into());
        assert_eq!(out.len(), 65);
        assert_eq!(out[0], "save-6");
    }
}
