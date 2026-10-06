/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::engine::GenericLspEngine;

pub const METHOD_NOT_FOUND: i64 = -32601;
pub const SEMANTIC_POLL: Duration = Duration::from_millis(300);
pub const FIRST_PUBLICATION_WAIT: Duration = Duration::from_secs(3);

pub(crate) struct Published {
    /// The document version the publication was for, when the server says (clangd does).
    pub(crate) version: Option<i64>,
    /// When it arrived.
    pub(crate) at: Instant,
    pub(crate) items: Vec<serde_json::Value>,
}

/// The text last sent for one document.
#[derive(Debug, Clone)]
pub(crate) struct Sent {
    pub(crate) version: Option<i64>,
    pub(crate) at: Instant,
}

/// Why a server has no report on a document's current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// No text of the document was sent, and the server published nothing for it.
    NeverPublished,
    /// A text was sent (with this version, when it had one), and the server has published
    /// nothing for the document since it was opened.
    NotPublished { sent: Option<i64> },
    /// The last publication is for a version older than the one last sent.
    OlderVersion { published: i64, sent: i64 },
    /// A publication names a version not sent for the current opening of the document.
    UnexpectedVersion { published: i64, sent: i64 },
    /// The last publication carries no version, from a server that numbers its publications:
    /// clangd publishes so for a document just closed.
    Unversioned { sent: Option<i64> },
    /// The last publication arrived before the text last sent. Without a version on both
    /// sides, the order of arrival is all that tells them apart.
    Earlier { sent: Option<i64> },
    /// The server exited.
    ServerExited,
}

/// No publication covers the text last sent for a document, so there is no report of its
/// diagnostics; which is not a report that it has none (#471).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsUnavailable {
    pub uri: String,
    /// How long the publication was waited for.
    pub waited: Duration,
    pub kind: Unavailable,
}

impl std::fmt::Display for DiagnosticsUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ms = self.waited.as_millis();
        let version = |v: &Option<i64>| v.map(|v| format!(" (version {v})")).unwrap_or_default();
        write!(f, "no current diagnostics for {}: ", self.uri)?;
        match &self.kind {
            Unavailable::NeverPublished => write!(
                f,
                "no text of it was sent, and the language server published nothing for it \
                 within {ms} ms"
            )?,
            Unavailable::NotPublished { sent } => write!(
                f,
                "the language server published nothing for the text last sent{} within {ms} ms",
                version(sent)
            )?,
            Unavailable::OlderVersion { published, sent } => write!(
                f,
                "the language server's last publication is for version {published}, older than \
                 version {sent} last sent, and none for version {sent} came within {ms} ms"
            )?,
            Unavailable::UnexpectedVersion { published, sent } => write!(
                f,
                "the language server published version {published}, but the current text is version \
                 {sent}; no matching report arrived within {ms} ms"
            )?,
            Unavailable::Unversioned { sent: None } => write!(
                f,
                "the language server numbers its publications, and its last one for the document \
                 carries no version, as one for a closed document does; no text of it is open"
            )?,
            Unavailable::Unversioned { sent } => write!(
                f,
                "the language server numbers its publications, and its last one for the document \
                 carries no version, as one for a closed document does; none for the text last \
                 sent{} came within {ms} ms",
                version(sent)
            )?,
            Unavailable::Earlier { sent } => write!(
                f,
                "the language server's last publication arrived before the text last sent{}, \
                 and none came after it within {ms} ms",
                version(sent)
            )?,
            Unavailable::ServerExited => write!(
                f,
                "the language server exited before it published for the text last sent"
            )?,
        }
        write!(
            f,
            "; there is no report, which does not mean the document has no errors"
        )
    }
}

impl std::error::Error for DiagnosticsUnavailable {}

/// Why a publication does not cover the text last sent for its document, or `None` when it
/// does: one for exactly that version, or, when either side has no version, one that
/// arrived after the text was sent. From a server that numbers its publications
/// (`versioned`), one without a number covers no numbered text, and with nothing sent it is a
/// closed document's; with nothing sent, any other publication covers the document.
pub(crate) fn gap(
    published: &Published,
    sent: Option<&Sent>,
    versioned: bool,
) -> Option<Unavailable> {
    match (published.version, sent) {
        (None, None) if versioned => Some(Unavailable::Unversioned { sent: None }),
        (_, None) => None,
        (
            Some(p),
            Some(Sent {
                version: Some(s), ..
            }),
        ) => match p.cmp(s) {
            std::cmp::Ordering::Less => Some(Unavailable::OlderVersion {
                published: p,
                sent: *s,
            }),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(Unavailable::UnexpectedVersion {
                published: p,
                sent: *s,
            }),
        },
        (
            None,
            Some(Sent {
                version: Some(s), ..
            }),
        ) if versioned => Some(Unavailable::Unversioned { sent: Some(*s) }),
        (_, Some(sent)) => {
            (published.at < sent.at).then_some(Unavailable::Earlier { sent: sent.version })
        }
    }
}

impl GenericLspEngine {
    pub async fn has_pull_diagnostics(&self) -> bool {
        self.capabilities
            .read()
            .await
            .as_ref()
            .and_then(|c| c.get("diagnosticProvider"))
            .is_some_and(|d| !d.is_null())
    }

    /// The document's diagnostics as the server computes them now, asked with a pull
    /// (`textDocument/diagnostic`), or `None` when the server does not answer one. It is asked
    /// whether or not it advertised the method: sourcekit-lsp answers a pull without
    /// advertising it, and what it publishes first for a document is an empty list, ahead of
    /// the check that finds the errors (#293). A server that does not know the method (clangd)
    /// is not asked again.
    pub async fn pull_diagnostics(&self, uri: &str) -> Option<Vec<serde_json::Value>> {
        if self.pull_unsupported.load(Ordering::Relaxed) {
            return None;
        }
        let answer = self
            .send_request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
            .ok()?;
        if let Some(err) = answer.get("error") {
            let code = err.get("code").and_then(|c| c.as_i64());
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
            if code == Some(METHOD_NOT_FOUND)
                || code == Some(-32603)
                || msg.to_lowercase().contains("unsupported")
                || msg.to_lowercase().contains("not implemented")
            {
                self.pull_unsupported.store(true, Ordering::Relaxed);
            }
            return None;
        }
        let kind = answer.pointer("/result/kind").and_then(|k| k.as_str());
        if kind == Some("full") {
            let items = answer.pointer("/result/items")?.as_array().cloned()?;
            let version = self.sent.read().await.get(uri).and_then(|s| s.version);
            let mut published = self.diagnostics.write().await;
            published.insert(
                uri.to_string(),
                Published {
                    version,
                    at: Instant::now(),
                    items: items.clone(),
                },
            );
            return Some(items);
        } else if kind == Some("unchanged") {
            let published = self.diagnostics.read().await;
            if let Some(p) = published.get(uri) {
                return Some(p.items.clone());
            }
        }
        None
    }

    /// Opens `text` as the file at `path` until the server reports an error on its 0-based
    /// `line`, closes it again, and says whether that happened within `timeout`. `text` is a
    /// real file of the project with a line appended that only a type check can fault, so a
    /// server that faults it checks files with the project's real build settings. sourcekit-lsp
    /// loads a package's settings in the background after it starts; until then it checks with
    /// fallback settings that report syntax errors only, and a check answered in that window
    /// found no errors at all (#295). `Ok(false)` means the server did report on the text and
    /// found nothing on that line; when it never reported on it at all, the error says why.
    pub async fn wait_for_semantic_check(
        &self,
        path: &Path,
        language_id: &str,
        text: &str,
        line: u64,
        timeout: Duration,
    ) -> Result<bool, DiagnosticsUnavailable> {
        let started = Instant::now();
        let unavailable = |uri: String, kind| DiagnosticsUnavailable {
            uri,
            waited: started.elapsed(),
            kind,
        };
        let Ok(uri) = url::Url::from_file_path(path) else {
            let shown = path.display().to_string();
            return Err(unavailable(shown, Unavailable::NeverPublished));
        };
        let uri = uri.to_string();
        let open = serde_json::json!({ "textDocument": {
            "uri": uri, "languageId": language_id, "version": 1, "text": text
        }});
        if self
            .send_notification("textDocument/didOpen", open)
            .await
            .is_err()
        {
            return Err(unavailable(uri, Unavailable::ServerExited));
        }
        let mut faulted = false;
        let mut reported = false;
        let mut missing = None;
        loop {
            let items = match self.pull_diagnostics(&uri).await {
                Some(items) => Some(items),
                None => match self.current_diagnostics_for(&uri, SEMANTIC_POLL).await {
                    Ok(items) => Some(items),
                    Err(err) => {
                        let exited = err.kind == Unavailable::ServerExited;
                        missing = Some(err);
                        if exited {
                            break;
                        }
                        None
                    }
                },
            };
            if let Some(items) = items {
                reported = true;
                faulted = items.iter().any(|d| {
                    d.get("severity").and_then(|s| s.as_u64()) == Some(1)
                        && d.pointer("/range/start/line").and_then(|l| l.as_u64()) == Some(line)
                });
            }
            if faulted || started.elapsed() >= timeout {
                break;
            }
            tokio::time::sleep(SEMANTIC_POLL).await;
        }
        let close = serde_json::json!({ "textDocument": { "uri": uri } });
        let _ = self.send_notification("textDocument/didClose", close).await;
        match missing {
            Some(err) if !reported => Err(err),
            _ => Ok(faulted),
        }
    }

    /// Whether the server has published diagnostics for `uri` at least once.
    pub async fn diagnostics_published(&self, uri: &str) -> bool {
        self.diagnostics.read().await.contains_key(uri)
    }

    /// The diagnostics the server last published for `uri`, whichever text they were for, or
    /// `None` when it has published none since the document was last opened or closed.
    pub async fn diagnostics_for(&self, uri: &str) -> Option<Vec<serde_json::Value>> {
        self.diagnostics
            .read()
            .await
            .get(uri)
            .map(|p| p.items.clone())
    }

    /// The diagnostics for the text last sent for `uri`. A server that pushes diagnostics
    /// publishes for each text it builds, and a publication for the text before the last
    /// change may still arrive after that change was sent; answering with it reports the old
    /// text's errors as the new one's (#293). This waits up to `wait` for a publication that
    /// covers the last text sent, and up to [`FIRST_PUBLICATION_WAIT`] for a first one when
    /// nothing was sent for the document. Past that, or once the server has exited, there is
    /// no report, and the error says why: an empty list would read as "no errors" (#471).
    pub async fn current_diagnostics_for(
        &self,
        uri: &str,
        wait: Duration,
    ) -> Result<Vec<serde_json::Value>, DiagnosticsUnavailable> {
        let started = Instant::now();
        loop {
            let (kind, has_published, known) = {
                let sent = self.sent.read().await;
                let published = self.diagnostics.read().await;
                let last_sent = sent.get(uri);
                let has_published = published.contains_key(uri);
                let kind = match (published.get(uri), last_sent) {
                    (Some(p), _) => {
                        match gap(p, last_sent, self.versioned.load(Ordering::Relaxed)) {
                            None => return Ok(p.items.clone()),
                            Some(kind) => kind,
                        }
                    }
                    (None, Some(s)) => Unavailable::NotPublished { sent: s.version },
                    (None, None) => Unavailable::NeverPublished,
                };
                (kind, has_published, last_sent.is_some())
            };
            let kind = if self.is_alive() {
                kind
            } else {
                Unavailable::ServerExited
            };
            let limit = if known || has_published {
                wait
            } else {
                wait.min(FIRST_PUBLICATION_WAIT)
            };
            if kind == Unavailable::ServerExited || started.elapsed() >= limit {
                let err = DiagnosticsUnavailable {
                    uri: uri.to_string(),
                    waited: started.elapsed(),
                    kind,
                };
                if has_published && known {
                    tracing::warn!(error = %err, "no diagnostics published for the text last sent");
                }
                return Err(err);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
