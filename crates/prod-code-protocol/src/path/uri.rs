/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::Value;
use std::path::{Path, PathBuf};
use url::Url;

/// The `file://` URI of an absolute `path`, percent-encoded so that spaces, `#`, `%` and
/// non-ASCII names survive the trip back. A relative path, which no file URI can name, keeps
/// the plain form.
pub fn file_uri(path: &Path) -> String {
    Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

/// The local path a `file:` URI names, decoded exactly once; `None` for another scheme or a
/// URI naming another host.
pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

/// The path a `file:` URI or a plain path names. A URI is decoded once; a plain path is taken
/// literally, so a file named `100%41.rs` stays that file.
pub fn uri_or_path(text: &str) -> PathBuf {
    if text.starts_with("file:") {
        file_uri_path(text).unwrap_or_else(|| PathBuf::from(text.trim_start_matches("file://")))
    } else {
        PathBuf::from(text)
    }
}

/// Fields whose strings are text — source, edits, documentation, messages — never a location.
pub(crate) const TEXT_FIELDS: &[&str] = &[
    "text",
    "newText",
    "insertText",
    "filterText",
    "sortText",
    "documentation",
    "contents",
    "value",
    "message",
    "label",
    "detail",
    "title",
    "tooltip",
];

/// Maps LSP location strings and file-URI object keys, leaving source and documentation text
/// alone. The mapper returns `None` for locations it does not own. This also lets the editor
/// mirror use the same payload boundaries as workspace translation. Returns whether anything
/// changed; a mapper that only observes locations can always return `None`.
pub fn map_lsp_locations(
    value: &mut Value,
    mapper: &mut impl FnMut(&str) -> Option<String>,
) -> bool {
    match value {
        Value::String(text) => match mapper(text) {
            Some(mapped) => {
                *text = mapped;
                true
            }
            None => false,
        },
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            map_lsp_locations(item, mapper) | changed
        }),
        Value::Object(map) => {
            let mut changed = false;
            let uri_keys: Vec<(String, String)> = map
                .keys()
                .filter(|key| key.starts_with("file:"))
                .filter_map(|key| mapper(key).map(|mapped| (key.clone(), mapped)))
                .collect();
            for (key, mapped) in uri_keys {
                if let Some(item) = map.remove(&key) {
                    map.insert(mapped, item);
                    changed = true;
                }
            }
            for (key, item) in map.iter_mut() {
                if !TEXT_FIELDS.contains(&key.as_str()) || (key == "label" && item.is_array()) {
                    changed |= map_lsp_locations(item, mapper);
                }
            }
            changed
        }
        _ => false,
    }
}
