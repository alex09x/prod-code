/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use ra_ap_ide::{TextRange, TextSize};
use ra_ap_vfs::FileId;
use serde_json::{Value, json};

use super::lines::{Lines, document};
use crate::RustEngineSnapshot;

impl RustEngineSnapshot {
    /// The edition the crate of `file_id` is written in, as rustfmt spells it.
    pub(crate) fn edition_of(&self, file_id: FileId) -> Option<String> {
        let krate = self.analysis.crates_for(file_id).ok()?.into_iter().next()?;
        Some(self.analysis.crate_edition(krate).ok()?.to_string())
    }

    /// The whole document formatted by rustfmt on this node, under the checkout's
    /// `rustfmt.toml`, as one edit; no edit when it is formatted already.
    pub fn formatting(&self, params: &Value) -> Result<Value> {
        let (path, file_id, text) = document(self, params)?;
        let mut command = std::process::Command::new("rustfmt");
        command.args(["--emit", "stdout", "--quiet"]);
        if let Some(edition) = self.edition_of(file_id) {
            command.args(["--edition", &edition]);
        }
        if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
            command.current_dir(dir);
        }
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("rustfmt did not start")?;
        let mut stdin = child.stdin.take().context("rustfmt has no stdin")?;
        let input = text.clone();
        let writer = std::thread::spawn(move || {
            use std::io::Write;
            stdin.write_all(input.as_bytes())
        });
        let output = child.wait_with_output().context("rustfmt did not finish")?;
        let _ = writer.join();
        if !output.status.success() {
            anyhow::bail!(
                "rustfmt failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let formatted = String::from_utf8(output.stdout).context("rustfmt printed no UTF-8")?;
        if formatted == text {
            return Ok(json!([]));
        }
        let lines = Lines::new(&text);
        Ok(json!([{
            "range": lines.range(TextRange::up_to(TextSize::of(text.as_str()))),
            "newText": formatted,
        }]))
    }
}
