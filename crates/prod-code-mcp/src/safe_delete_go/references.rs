/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, anyhow};

use super::directives::{current_text, regular_unlinked_inside};
use super::types::{FunctionRange, display, line_col_utf16, lsp_span};

pub(crate) async fn reference_evidence(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    function: &FunctionRange,
) -> Result<()> {
    let (line, character) = line_col_utf16(text, function.name_start)?;
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow!("invalid Go source path {}", file.display()))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true }
    });
    let mut answer = serde_json::Value::Null;
    for attempt in 0..=crate::impact::COLD_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(crate::impact::COLD_WAIT).await;
        }
        answer = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/references",
            params.clone(),
        )
        .await
        .context("gopls could not list every reference")?;
        if answer
            .as_array()
            .is_some_and(|locations| !locations.is_empty())
        {
            break;
        }
    }
    let locations = answer
        .as_array()
        .filter(|locations| !locations.is_empty())
        .with_context(|| {
            format!(
                "gopls listed no location, not even the declaration of {}: {answer}",
                function.name
            )
        })?;

    let mut declarations = 0usize;
    let mut uses = Vec::new();
    let mut seen = BTreeSet::new();
    for location in locations {
        let uri = location
            .get("uri")
            .and_then(serde_json::Value::as_str)
            .context("a reference has no file URI")?;
        let uri = url::Url::parse(uri).context("a reference has an invalid URI")?;
        anyhow::ensure!(
            uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
            "a reference is not a plain local file URI: {uri}"
        );
        let reported = uri
            .to_file_path()
            .map_err(|_| anyhow!("a reference is not a local file URI: {uri}"))?;
        let path = regular_unlinked_inside(root, &reported)?;
        let source = if path == file {
            text.to_string()
        } else {
            std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read referenced source {}", path.display()))?
        };
        let span = lsp_span(
            &source,
            location.get("range").context("a reference has no range")?,
        )
        .context("a reference has a malformed range")?;
        anyhow::ensure!(
            source.get(span.start..span.end) == Some(function.name.as_str())
                && span.end - span.start == function.name.len(),
            "{} names stale or malformed reference evidence",
            display(root, &path)
        );
        anyhow::ensure!(
            seen.insert((path.clone(), span.start)),
            "{} contains duplicate reference evidence",
            display(root, &path)
        );
        if path == file && span.start == function.name_start && span.end == function.name_end {
            declarations += 1;
        } else {
            let (line, col) = line_col_utf16(&source, span.start)?;
            uses.push(format!("{}:{}:{}", display(root, &path), line + 1, col + 1));
        }
    }
    anyhow::ensure!(
        declarations == 1,
        "gopls listed the declaration {} times instead of exactly once",
        declarations
    );
    anyhow::ensure!(
        uses.is_empty(),
        "{} is still referenced at {}; no function was deleted",
        function.name,
        uses.join(", ")
    );
    anyhow::ensure!(
        current_text(file, text)?,
        "{} changed while its references were inspected",
        display(root, file)
    );
    Ok(())
}
