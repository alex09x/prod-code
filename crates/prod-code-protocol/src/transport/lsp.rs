/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Maximum aggregate size of the textual headers of one language-server message.
const MAX_LSP_HEADER_BYTES: usize = 64 * 1024;

/// Reads one complete UTF-8 LSP frame, including all headers before the blank line.
///
/// Clean EOF between frames returns `None`. Malformed or truncated input is an error:
/// callers must retire the stream rather than try to find a new frame inside its body.
/// Headers are limited to 64 KiB and the body to the wire codec's 256 MiB limit.
pub async fn read_lsp_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    read_lsp_frame_with_limits(reader, MAX_LSP_HEADER_BYTES, crate::codec::MAX_FRAME_SIZE).await
}

pub(crate) async fn read_lsp_frame_with_limits<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max_headers: usize,
    max_body: usize,
) -> std::io::Result<Option<String>> {
    use std::io::{Error, ErrorKind};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let invalid = |message| Error::new(ErrorKind::InvalidData, message);
    let truncated = |message| Error::new(ErrorKind::UnexpectedEof, message);
    let mut header_bytes = 0;
    let mut length = None;
    let mut has_content_type = false;
    loop {
        let remaining = max_headers - header_bytes;
        if remaining == 0 {
            return Err(invalid("LSP headers exceed the size limit"));
        }
        let mut line = Vec::new();
        // Limit the read itself: a child can emit an endless line with no newline.
        let count = (&mut *reader)
            .take(remaining as u64)
            .read_until(b'\n', &mut line)
            .await?;
        if count == 0 {
            return if header_bytes == 0 {
                Ok(None)
            } else {
                Err(truncated("EOF inside LSP headers"))
            };
        }
        if line.last() != Some(&b'\n') {
            return Err(if count == remaining {
                invalid("LSP headers exceed the size limit")
            } else {
                truncated("EOF inside an LSP header line")
            });
        }
        header_bytes += count;
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            break;
        }
        if !line.is_ascii() {
            return Err(invalid("LSP headers must be ASCII"));
        }
        let line = std::str::from_utf8(&line).expect("ASCII was checked");
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("Malformed LSP header"))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(invalid("Duplicate LSP Content-Length"));
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid("Invalid LSP Content-Length"));
            }
            let parsed = value
                .parse::<usize>()
                .map_err(|_| invalid("Invalid LSP Content-Length"))?;
            if parsed == 0 || parsed > max_body {
                return Err(invalid("LSP body length exceeds the allowed range"));
            }
            length = Some(parsed);
        } else if name.eq_ignore_ascii_case("Content-Type") {
            if has_content_type {
                return Err(invalid("Duplicate LSP Content-Type"));
            }
            has_content_type = true;
            for parameter in value.split(';').skip(1) {
                if let Some((key, encoding)) = parameter.trim().split_once('=')
                    && key.trim().eq_ignore_ascii_case("charset")
                {
                    let encoding = encoding.trim().trim_matches('"');
                    if !encoding.eq_ignore_ascii_case("utf-8")
                        && !encoding.eq_ignore_ascii_case("utf8")
                    {
                        return Err(invalid("LSP bodies must use UTF-8"));
                    }
                }
            }
        }
    }
    let length = length.ok_or_else(|| invalid("Missing LSP Content-Length"))?;
    // Grow only as bytes arrive, rather than preallocating a child's declared length.
    let mut body = Vec::new();
    (&mut *reader)
        .take(length as u64)
        .read_to_end(&mut body)
        .await?;
    if body.len() != length {
        return Err(truncated("EOF inside LSP body"));
    }
    String::from_utf8(body)
        .map(Some)
        .map_err(|_| invalid("LSP body is not valid UTF-8"))
}
