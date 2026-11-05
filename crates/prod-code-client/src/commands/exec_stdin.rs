/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `exec` runs argv on a node and does not carry standard input there (#1944). A heredoc or a pipe
//! given to it used to vanish: `exec -- python3 - <<'PY' ...` ran an empty program and reported
//! success. The CLI now says so before anything runs.

use std::io::{IsTerminal, Read};
use std::sync::mpsc;
use std::time::Duration;

/// How long to wait for a first byte. Data that is already there (a heredoc, a file, a pipe whose
/// writer has finished) arrives at once; a pipe that is merely open and silent does not count.
const GRACE: Duration = Duration::from_millis(300);

/// True when `reader` yields at least one byte within `grace`. An end of input, an error or
/// silence is false. The probing thread may stay blocked on a silent reader; it holds nothing else.
pub fn reader_has_data<R: Read + Send + 'static>(mut reader: R, grace: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let _ = tx.send(matches!(reader.read(&mut byte), Ok(1)));
    });
    rx.recv_timeout(grace).unwrap_or(false)
}

/// Refuse an `exec` that was given standard input, which would be dropped.
pub fn refuse_unforwarded_stdin() -> anyhow::Result<()> {
    if std::io::stdin().is_terminal() || !reader_has_data(std::io::stdin(), GRACE) {
        return Ok(());
    }
    anyhow::bail!(
        "exec does not forward standard input to the node, so the heredoc or pipe given here would \
         be dropped and the command would run on empty input. Put the program in a file in the \
         checkout, or pass it as an argument (`python3 -c '...'`, `sh -c '...'`), and run that"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn input_that_is_already_there_is_seen() {
        let data = Cursor::new(b"print('stdin-marker')\n".to_vec());
        assert!(reader_has_data(data, Duration::from_secs(5)));
    }

    #[test]
    fn an_empty_input_is_not_data() {
        assert!(!reader_has_data(
            Cursor::new(Vec::new()),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn a_silent_open_pipe_is_not_data_and_does_not_hold_the_caller() {
        struct Silent(mpsc::Receiver<()>);
        impl Read for Silent {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                let _ = self.0.recv();
                Ok(0)
            }
        }
        let (keep_open, rx) = mpsc::channel();
        let started = std::time::Instant::now();
        assert!(!reader_has_data(Silent(rx), Duration::from_millis(50)));
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(keep_open);
    }

    #[test]
    fn a_failing_reader_is_not_data() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("closed"))
            }
        }
        assert!(!reader_has_data(Broken, Duration::from_secs(5)));
    }
}
