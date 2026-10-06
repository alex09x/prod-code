/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::messages::WireMessage;
use bytes::{BufMut, BytesMut};
use std::io;

/// Maximum allowed wire frame size. A sync message carries at most 24 MiB of content, but a
/// vendored library a build links (up to 128 MiB, base64 on the wire) travels in a message of
/// its own (#313).
pub const MAX_FRAME_SIZE: usize = 256 * 1024 * 1024;

/// Length-delimited codec for `WireMessage`, optionally with trailing NUL completion marker.
///
/// Format on wire:
/// `[4-byte big-endian length N] [N bytes UTF-8 JSON encoded WireMessage] [optional 0x00 NUL marker]`
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProdCodeCodec {
    /// When true, outgoing frames will append a trailing NUL byte (0x00) after the frame body,
    /// and incoming frames will require the NUL completion marker.
    pub(crate) nul_marker: bool,
    /// When decoding without `nul_marker` explicitly configured, tracks whether the peer
    /// stream has been confirmed to emit trailing NUL completion markers.
    pub(crate) detected_nul_marker: bool,
    /// When a frame was decoded without consuming a trailing NUL marker (e.g. before NUL
    /// detection was confirmed), records that a trailing NUL marker may follow in the stream.
    pub(crate) pending_trailing_nul: bool,
}

pub(crate) struct BoundedWriter<W> {
    inner: W,
    written: usize,
    limit: usize,
}

impl<W: io::Write> BoundedWriter<W> {
    pub(crate) fn new(inner: W, limit: usize) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }
}

impl<W: io::Write> io::Write for BoundedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(buf.len()) > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Message size exceeds maximum allowed frame size {}",
                    self.limit
                ),
            ));
        }
        let n = self.inner.write(buf)?;
        self.written += n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl ProdCodeCodec {
    pub fn new() -> Self {
        Self {
            nul_marker: false,
            detected_nul_marker: false,
            pending_trailing_nul: false,
        }
    }

    /// Creates a codec configured with or without trailing NUL completion markers.
    pub fn with_nul_marker(mut self, enabled: bool) -> Self {
        self.nul_marker = enabled;
        self.detected_nul_marker = enabled;
        self.pending_trailing_nul = false;
        self
    }

    /// Whether this codec is configured to emit and enforce trailing NUL completion markers.
    pub fn has_nul_marker(&self) -> bool {
        self.nul_marker || self.detected_nul_marker
    }

    /// Encode a referenced `WireMessage` directly into `dst` without allocating an intermediate heap buffer.
    pub fn encode_ref(&self, item: &WireMessage, dst: &mut BytesMut) -> Result<(), io::Error> {
        self.encode_ref_bounded(item, dst, MAX_FRAME_SIZE)
    }

    pub(crate) fn encode_ref_bounded(
        &self,
        item: &WireMessage,
        dst: &mut BytesMut,
        max_frame_size: usize,
    ) -> Result<(), io::Error> {
        if let WireMessage::HttpResponse {
            status,
            content_type,
            body,
        } = item
        {
            let status_text = match *status {
                200 => "OK",
                400 => "Bad Request",
                404 => "Not Found",
                426 => "Upgrade Required",
                _ => "OK",
            };
            let response = format!(
                "HTTP/1.1 {status} {status_text}\r\n\
                 Content-Type: {content_type}\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                body.len()
            );
            dst.extend_from_slice(response.as_bytes());
            return Ok(());
        }

        let initial_capacity = dst.capacity();
        let header_offset = dst.len();
        dst.reserve(4);
        dst.put_u32(0); // Placeholder for 4-byte BE length

        // Stream JSON directly into dst's spare capacity using serde_json::to_writer
        // bounded by BoundedWriter to abort serialization as soon as the payload exceeds max_frame_size.
        let writer = BoundedWriter::new((&mut *dst).writer(), max_frame_size);
        if let Err(e) = serde_json::to_writer(writer, item) {
            dst.truncate(header_offset);
            if dst.capacity() > initial_capacity {
                let mut restored = BytesMut::with_capacity(initial_capacity);
                restored.put_slice(&dst[..header_offset]);
                *dst = restored;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to serialize WireMessage: {e}"),
            ));
        }

        let frame_len = dst.len() - header_offset - 4;
        if frame_len > max_frame_size {
            dst.truncate(header_offset);
            if dst.capacity() > initial_capacity {
                let mut restored = BytesMut::with_capacity(initial_capacity);
                restored.put_slice(&dst[..header_offset]);
                *dst = restored;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Message size {frame_len} exceeds maximum {max_frame_size}"),
            ));
        }

        dst[header_offset..header_offset + 4].copy_from_slice(&(frame_len as u32).to_be_bytes());
        if self.nul_marker {
            dst.reserve(1);
            dst.put_u8(0);
        }
        Ok(())
    }
}
