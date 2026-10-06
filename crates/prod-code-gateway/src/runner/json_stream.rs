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

#[derive(Default)]
pub struct JsonStreamAccumulator {
    pub stdout_line_buf: String,
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
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Ok(true);
        };
        self.stdout_line_buf.push_str(text);
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
