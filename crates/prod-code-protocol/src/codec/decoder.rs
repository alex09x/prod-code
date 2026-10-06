/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{MAX_FRAME_SIZE, ProdCodeCodec};
use crate::messages::WireMessage;
use bytes::{Buf, BytesMut};
use std::io;
use tokio_util::codec::Decoder;

/// RFC 9110 Section 5.6.2 token character:
/// `tchar = "!" / "#" / "$" / "%" / "&" / "'" / "*" / "+" / "-" / "." / "^" / "_" / "`" / "|" / "~" / DIGIT / ALPHA`
#[inline]
fn is_rfc_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

impl Decoder for ProdCodeCodec {
    type Item = WireMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if self.pending_trailing_nul {
            if src.is_empty() {
                return Ok(None);
            }
            if src[0] != 0 {
                // The byte immediately following the previous frame is non-zero, so the stream
                // is confirmed to be markerless.
                self.pending_trailing_nul = false;
            } else {
                // src[0] == 0: could be the trailing NUL marker from the previous frame,
                // or the first byte of a markerless frame with length < 16 MiB.
                // We need at least 5 bytes to inspect the next frame's 4-byte header under
                // the trailing-NUL hypothesis (1 byte NUL + 4 bytes header).
                if src.len() < 5 {
                    return Ok(None);
                }

                let len_a = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;
                let len_b = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;

                if len_b < 2 {
                    // Hypothesis B is impossible because no valid WireMessage payload has length < 2.
                    // Therefore, src[0] is definitely a trailing NUL marker.
                    src.advance(1);
                    self.detected_nul_marker = true;
                    self.pending_trailing_nul = false;
                } else if len_a > MAX_FRAME_SIZE {
                    self.pending_trailing_nul = false;
                } else {
                    let hyp_a_valid = if src.len() >= 5 + len_a {
                        Some(serde_json::from_slice::<WireMessage>(&src[5..5 + len_a]).is_ok())
                    } else {
                        None
                    };

                    let hyp_b_valid = if src.len() >= 4 + len_b {
                        Some(serde_json::from_slice::<WireMessage>(&src[4..4 + len_b]).is_ok())
                    } else {
                        None
                    };

                    match (hyp_a_valid, hyp_b_valid) {
                        (Some(true), _) => {
                            src.advance(1);
                            self.detected_nul_marker = true;
                            self.pending_trailing_nul = false;
                        }
                        (_, Some(true)) => {
                            self.pending_trailing_nul = false;
                        }
                        (Some(false), Some(false)) => {
                            self.pending_trailing_nul = false;
                        }
                        (Some(false), None) => {
                            self.pending_trailing_nul = false;
                        }
                        (None, Some(false)) => {
                            src.advance(1);
                            self.detected_nul_marker = true;
                            self.pending_trailing_nul = false;
                        }
                        (None, None) => {
                            // Partial ambiguous bytes; wait for more data to confirm boundary
                            return Ok(None);
                        }
                    }
                }
            }
        }

        if src.len() < 4 {
            return Ok(None);
        }

        // Check if incoming stream begins with a syntactically valid HTTP request line or response.
        let is_http = if src.starts_with(b"HTTP/") {
            true
        } else if let Some(first_space) = src.iter().position(|&b| b == b' ') {
            if (1..=64).contains(&first_space)
                && src[..first_space].iter().all(|&b| is_rfc_token_byte(b))
            {
                let rest = &src[first_space + 1..];
                if rest.is_empty() {
                    true
                } else if let Some(second_space) = rest.iter().position(|&b| b == b' ') {
                    let target = &rest[..second_space];
                    let after_second = &rest[second_space + 1..];
                    !target.is_empty()
                        && target.iter().all(|&b| b > 0x20 && b < 0x7F)
                        && (after_second.is_empty()
                            || after_second.starts_with(b"HTTP/")
                            || b"HTTP/".starts_with(&after_second[..after_second.len().min(5)]))
                } else if let Some(line_end) = rest.windows(2).position(|w| w == b"\r\n") {
                    let target = &rest[..line_end];
                    !target.is_empty() && target.iter().all(|&b| b > 0x20 && b < 0x7F)
                } else if let Some(line_end) = rest.iter().position(|&b| b == b'\n') {
                    let target = &rest[..line_end];
                    !target.is_empty() && target.iter().all(|&b| b > 0x20 && b < 0x7F)
                } else {
                    rest.iter().all(|&b| b > 0x20 && b < 0x7F)
                }
            } else {
                false
            }
        } else {
            src.len() <= 64 && src.iter().all(|&b| is_rfc_token_byte(b))
        };

        if is_http {
            let header_end = src
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .or_else(|| src.windows(2).position(|w| w == b"\n\n").map(|p| p + 2));

            match header_end {
                Some(end) => {
                    let req_bytes = src.split_to(end);
                    let req_str = String::from_utf8_lossy(&req_bytes);
                    let mut lines = req_str.lines();
                    let first_line = lines.next().unwrap_or("");
                    let mut parts = first_line.split_whitespace();
                    let method = parts.next().unwrap_or("GET").to_string();
                    let path = parts.next().unwrap_or("/").to_string();

                    return Ok(Some(WireMessage::HttpProbe { method, path }));
                }
                None => {
                    if src.len() > 8192 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "HTTP headers exceeded maximum limit for probe",
                        ));
                    }
                    return Ok(None);
                }
            }
        }

        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&src[..4]);
        let frame_len = u32::from_be_bytes(length_bytes) as usize;

        if frame_len > MAX_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Frame size {frame_len} exceeds maximum {MAX_FRAME_SIZE}"),
            ));
        }

        let uses_nul = self.nul_marker || self.detected_nul_marker;

        if uses_nul {
            if src.len() < 4 + frame_len + 1 {
                return Ok(None);
            }

            src.advance(4);
            let mut frame_data = src.split_to(frame_len);

            if src.first() == Some(&0) {
                src.advance(1);
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Missing expected NUL completion marker after frame",
                ));
            }

            if frame_data.ends_with(b"\0") {
                frame_data.truncate(frame_data.len() - 1);
            }

            let message = serde_json::from_slice::<WireMessage>(&frame_data).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to parse WireMessage: {e}"),
                )
            })?;

            return Ok(Some(message));
        }

        if src.len() < 4 + frame_len {
            return Ok(None);
        }

        src.advance(4);
        let mut frame_data = src.split_to(frame_len);

        if src.is_empty() {
            self.pending_trailing_nul = true;
        } else if src[0] != 0 {
            self.pending_trailing_nul = false;
        } else if src.len() >= 5 {
            let hyp_a_valid = {
                let len_a = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
                if !(2..=MAX_FRAME_SIZE).contains(&len_a) {
                    Some(false)
                } else if src.len() >= 4 + len_a {
                    Some(serde_json::from_slice::<WireMessage>(&src[4..4 + len_a]).is_ok())
                } else {
                    None
                }
            };

            let hyp_b_valid = {
                let len_b = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;
                if !(2..=MAX_FRAME_SIZE).contains(&len_b) {
                    Some(false)
                } else if src.len() >= 5 + len_b {
                    Some(serde_json::from_slice::<WireMessage>(&src[5..5 + len_b]).is_ok())
                } else {
                    None
                }
            };

            match (hyp_a_valid, hyp_b_valid) {
                (_, Some(true)) | (Some(false), None) => {
                    src.advance(1);
                    self.detected_nul_marker = true;
                    self.pending_trailing_nul = false;
                }
                (Some(true), _) | (None, Some(false)) => {
                    self.pending_trailing_nul = false;
                    self.detected_nul_marker = false;
                }
                _ => {
                    self.pending_trailing_nul = true;
                }
            }
        } else {
            self.pending_trailing_nul = true;
        }

        if frame_data.ends_with(b"\0") {
            frame_data.truncate(frame_data.len() - 1);
        }

        let message = serde_json::from_slice::<WireMessage>(&frame_data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse WireMessage: {e}"),
            )
        })?;

        Ok(Some(message))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(frame) => Ok(Some(frame)),
            None => {
                if self.pending_trailing_nul && src.len() == 1 && src[0] == 0 {
                    src.advance(1);
                    self.pending_trailing_nul = false;
                } else if !self.nul_marker && !self.detected_nul_marker && src.len() >= 4 {
                    let mut length_bytes = [0u8; 4];
                    length_bytes.copy_from_slice(&src[..4]);
                    let frame_len = u32::from_be_bytes(length_bytes) as usize;
                    if frame_len <= MAX_FRAME_SIZE
                        && src.len() == 4 + frame_len + 1
                        && src[4 + frame_len] == 0
                    {
                        src.advance(4);
                        let mut frame_data = src.split_to(frame_len);
                        src.advance(1); // consume trailing NUL at EOF
                        if frame_data.ends_with(b"\0") {
                            frame_data.truncate(frame_data.len() - 1);
                        }
                        let message =
                            serde_json::from_slice::<WireMessage>(&frame_data).map_err(|e| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!("Failed to parse WireMessage: {e}"),
                                )
                            })?;
                        return Ok(Some(message));
                    }
                }
                if src.is_empty() {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("Unexpected EOF with {} unparsed bytes", src.len()),
                    ))
                }
            }
        }
    }
}
