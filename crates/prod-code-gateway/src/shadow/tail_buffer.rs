/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) fn is_diagnostic_error_header(line: &str) -> bool {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("error") {
        let is_code = rest.starts_with('[') && rest.contains("]:");
        let is_colon = rest.starts_with(':');
        if is_code || is_colon {
            let msg = rest.split_once(':').map_or("", |(_, m)| m.trim());
            if !msg.starts_with("could not compile")
                && !msg.starts_with("aborting due to")
                && !msg.starts_with("build failed")
            {
                return true;
            }
        }
    }
    if trimmed.contains(": error:") || trimmed.contains(" - error TS") {
        return true;
    }
    (trimmed.starts_with("---- ") && trimmed.ends_with(" stdout ----"))
        || (trimmed.starts_with("thread '") && trimmed.contains("' panicked at "))
        || trimmed.starts_with("--- FAIL: ")
        || trimmed.starts_with("FAILED ")
}

pub(crate) fn is_diagnostic_boundary(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("warning:")
        || (trimmed.starts_with("warning[") && trimmed.contains("]:"))
        || trimmed.contains(": warning:")
        || trimmed.contains(" - warning TS")
        || is_diagnostic_error_header(line)
        || trimmed.starts_with("Compiling ")
        || trimmed.starts_with("Checking ")
        || trimmed.starts_with("Finished ")
        || trimmed.starts_with("error: could not compile")
        || trimmed.starts_with("error: aborting due to")
}

/// Keeps the last `limit` bytes of a stream, counts everything that went through, and retains
/// compiler error sections so they are not dropped when warnings exceed the tail cap (#794).
pub struct TailBuffer {
    limit: usize,
    buf: Vec<u8>,
    pub total: u64,
    retained_errors: Vec<u8>,
    line_buf: Vec<u8>,
    recording_error: bool,
}

impl TailBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            buf: Vec::new(),
            total: 0,
            retained_errors: Vec::new(),
            line_buf: Vec::new(),
            recording_error: false,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.total += data.len() as u64;
        self.buf.extend_from_slice(data);
        if self.buf.len() > self.limit {
            let cut = self.buf.len() - self.limit;
            self.buf.drain(..cut);
        }

        for &b in data {
            self.line_buf.push(b);
            if b == b'\n' {
                let line_str = String::from_utf8_lossy(&self.line_buf);
                if is_diagnostic_error_header(&line_str) {
                    self.recording_error = true;
                } else if self.recording_error && is_diagnostic_boundary(&line_str) {
                    self.recording_error = false;
                }

                if self.recording_error && self.retained_errors.len() < self.limit {
                    self.retained_errors.extend_from_slice(&self.line_buf);
                }
                self.line_buf.clear();
            }
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn to_output(&self) -> Vec<u8> {
        if self.total <= self.limit as u64 || self.retained_errors.is_empty() {
            return self.buf.clone();
        }
        let first_line = self
            .retained_errors
            .split(|&b| b == b'\n')
            .next()
            .unwrap_or(&[]);
        if !first_line.is_empty() && self.buf.windows(first_line.len()).any(|w| w == first_line) {
            return self.buf.clone();
        }
        let mut out = Vec::with_capacity(self.retained_errors.len() + 32 + self.buf.len());
        out.extend_from_slice(&self.retained_errors);
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        out.extend_from_slice(b"\n[... output truncated ...]\n");
        out.extend_from_slice(&self.buf);
        out
    }
}
