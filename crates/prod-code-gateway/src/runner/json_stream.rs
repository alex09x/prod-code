/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// The text of `pending` up to a character a later chunk has yet to complete, removed from it.
/// Bytes that are not UTF-8 at all are dropped, as before, and only they.
fn take_complete_text(pending: &mut Vec<u8>) -> String {
    let mut text = String::new();
    let mut at = 0;
    while at < pending.len() {
        match std::str::from_utf8(&pending[at..]) {
            Ok(rest) => {
                text.push_str(rest);
                at = pending.len();
            }
            Err(e) => {
                let valid = at + e.valid_up_to();
                text.push_str(std::str::from_utf8(&pending[at..valid]).unwrap_or_default());
                match e.error_len() {
                    Some(bad) => at = valid + bad,
                    // An incomplete character at the end: kept for the next chunk.
                    None => {
                        at = valid;
                        break;
                    }
                }
            }
        }
    }
    pending.drain(..at);
    text
}

#[cfg(test)]
#[test]
fn a_character_split_across_chunks_is_kept() {
    let line = "{\"name\":\"é\"}\n".as_bytes();
    let split = line.iter().position(|b| *b >= 0x80).unwrap() + 1;
    let mut pending = line[..split].to_vec();
    let first = take_complete_text(&mut pending);
    pending.extend_from_slice(&line[split..]);
    let second = take_complete_text(&mut pending);
    assert_eq!(format!("{first}{second}"), "{\"name\":\"é\"}\n");
    assert!(pending.is_empty());
    // Bytes that are not UTF-8 at all are dropped, and only they.
    let mut bad = b"ok\xff\xfe\n".to_vec();
    assert_eq!(take_complete_text(&mut bad), "ok\n");
    assert!(bad.is_empty());
}

#[derive(Default)]
pub struct JsonStreamAccumulator {
    pub stdout_line_buf: String,
    /// The tail of the last chunk that ended inside a multi-byte character.
    pending_bytes: Vec<u8>,
    pub diagnostics: Vec<prod_code_protocol::RemoteExecDiagnostic>,
    pub tests_passed: u64,
    pub tests_failed: u64,
    pub tests_skipped: u64,
    pub test_failures: Vec<RemoteExecTestEvent>,
    pub benches: Vec<RemoteExecTestEvent>,
}

impl JsonStreamAccumulator {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn push_chunk(
        &mut self,
        bytes: &[u8],
        language: RemoteExecLanguage,
        framed: &mut Framed<AnyStream, ProdCodeCodec>,
    ) -> Result<bool> {
        // A chunk may end inside a multi-byte character: its bytes wait for the next chunk
        // instead of failing to decode and taking whole records with them.
        self.pending_bytes.extend_from_slice(bytes);
        let text = take_complete_text(&mut self.pending_bytes);
        self.stdout_line_buf.push_str(&text);
        while let Some(pos) = self.stdout_line_buf.find('\n') {
            if pos > MAX_JSON_LINE_BUFFER_BYTES {
                tracing::warn!(
                    line_len = pos,
                    "stdout line exceeded limit of {MAX_JSON_LINE_BUFFER_BYTES} bytes; dropping unparsed line"
                );
                self.stdout_line_buf.drain(..=pos);
                continue;
            }
            let line = self.stdout_line_buf[..pos].trim_end().to_string();
            self.stdout_line_buf.drain(..=pos);
            if line.is_empty() {
                continue;
            }
            let stream_event = match language {
                RemoteExecLanguage::Rust => parse_cargo_json_event(&line),
                RemoteExecLanguage::Go => parse_go_test_json_event(&line),
                _ => None,
            };
            if let Some(ev) = stream_event {
                match &ev {
                    RemoteExecStream::Diagnostic(diag) => {
                        self.diagnostics.push(diag.clone());
                    }
                    RemoteExecStream::TestEvent(test_ev) => match test_ev {
                        RemoteExecTestEvent::Passed { .. } => self.tests_passed += 1,
                        RemoteExecTestEvent::Failed { .. } => {
                            self.tests_failed += 1;
                            self.test_failures.push(test_ev.clone());
                        }
                        RemoteExecTestEvent::Skipped { .. } => self.tests_skipped += 1,
                        RemoteExecTestEvent::Bench { .. } => self.benches.push(test_ev.clone()),
                        _ => {}
                    },
                    _ => {}
                }
                if framed
                    .send(WireMessage::RemoteExecStream(ev))
                    .await
                    .is_err()
                {
                    return Ok(false);
                }
            }
        }
        if self.stdout_line_buf.len() > MAX_JSON_LINE_BUFFER_BYTES {
            tracing::warn!(
                buf_len = self.stdout_line_buf.len(),
                "stdout buffer without newline exceeded limit of {MAX_JSON_LINE_BUFFER_BYTES} bytes; dropping unparsed buffer"
            );
            self.stdout_line_buf.clear();
        }
        Ok(true)
    }

    pub(crate) async fn flush(
        &mut self,
        language: RemoteExecLanguage,
        framed: &mut Framed<AnyStream, ProdCodeCodec>,
    ) -> Result<bool> {
        if self.stdout_line_buf.is_empty() {
            return Ok(true);
        }
        let line = self.stdout_line_buf.trim_end().to_string();
        if !line.is_empty() && line.len() <= MAX_JSON_LINE_BUFFER_BYTES {
            let stream_event = match language {
                RemoteExecLanguage::Rust => parse_cargo_json_event(&line),
                RemoteExecLanguage::Go => parse_go_test_json_event(&line),
                _ => None,
            };
            if let Some(ev) = stream_event {
                match &ev {
                    RemoteExecStream::Diagnostic(diag) => self.diagnostics.push(diag.clone()),
                    RemoteExecStream::TestEvent(test_ev) => match test_ev {
                        RemoteExecTestEvent::Passed { .. } => self.tests_passed += 1,
                        RemoteExecTestEvent::Failed { .. } => {
                            self.tests_failed += 1;
                            self.test_failures.push(test_ev.clone());
                        }
                        RemoteExecTestEvent::Skipped { .. } => self.tests_skipped += 1,
                        RemoteExecTestEvent::Bench { .. } => self.benches.push(test_ev.clone()),
                        _ => {}
                    },
                    _ => {}
                }
                if framed
                    .send(WireMessage::RemoteExecStream(ev))
                    .await
                    .is_err()
                {
                    return Ok(false);
                }
            }
        }
        self.stdout_line_buf.clear();
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
