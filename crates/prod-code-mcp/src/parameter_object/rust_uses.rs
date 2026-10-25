/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::ops::Range;
use std::path::Path;

use anyhow::{Context, Result};

use super::apply::{display, unlisted};
use super::drop::name_in;
use super::rust_edits::Edit;
use crate::signature::{Declared, offset_of, position_at, references};

/// The analyzer identifies bindings, while byte ranges distinguish a declaration from uses
/// later on that very same line. Untrusted positions remain unmatched and block writes.
#[allow(clippy::too_many_arguments)]
pub(super) async fn collect(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    signature: Range<usize>,
    declared: &[Declared],
    bundled: &[usize],
    binding: &str,
) -> Result<(Vec<Edit>, Vec<String>)> {
    let (mut edits, mut unmatched) = (Vec::new(), Vec::new());
    for i in bundled {
        let d = &declared[*i];
        let at = text[signature.clone()]
            .find(&d.raw)
            .map(|o| signature.start + o)
            .context("parameter declaration not found; nothing was written")?;
        let at = at + name_in(&d.raw, &d.name).context("parameter name not found")?;
        let (l, c) = position_at(text, at)?;
        let refs = references(remote, root, file, l, c)
            .await
            .with_context(|| unlisted(&d.name, root, file, l, c))?;
        for (path, rl, rc) in refs {
            let place = format!("{}:{rl}:{rc}", display(root, &path));
            let Some(o) = (path == file).then(|| offset_of(text, rl, rc)).flatten() else {
                unmatched.push(format!(
                    "{place} (no such local parameter reference position)"
                ));
                continue;
            };
            if o == at {
                continue;
            }
            let identifier = |c| c == '_' || unicode_ident::is_xid_continue(c);
            if !text[o..].starts_with(&d.name)
                || text[..o].chars().next_back().is_some_and(identifier)
                || text[o + d.name.len()..]
                    .chars()
                    .next()
                    .is_some_and(identifier)
                || signature.contains(&o)
            {
                unmatched.push(format!(
                    "{place} (parameter reference does not name `{}`)",
                    d.name
                ));
                continue;
            }
            let edit = Edit::text(o, d.name.len(), format!("{binding}.{}", d.name));
            if !edits.contains(&edit) {
                edits.push(edit);
            }
        }
    }
    Ok((edits, unmatched))
}
