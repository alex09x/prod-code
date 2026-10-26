/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use super::helpers::{is_ident, type_spelling};

const DEF: &str = "textDocument/definition";
const TYPE_DEF: &str = "textDocument/typeDefinition";

async fn query(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
    method: &str,
) -> Option<Value> {
    let (line, col) = crate::signature::position_at(text, at).ok()?;
    crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        method,
        json!({
            "textDocument": { "uri": url::Url::from_file_path(file).ok()?.to_string() },
            "position": { "line": line - 1, "character": col - 1 }
        }),
    )
    .await
    .ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnerIdentity {
    head: Value,
    targets: Vec<Value>,
}

/// Prove the nominal head definition and any ADT generic argument definitions.
pub(super) async fn owner_identity(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    self_type_at: usize,
    self_type: &str,
) -> Option<OwnerIdentity> {
    use crate::fixture::builder::lexer::{Kind, Source};

    let nominal = self_type.split('<').next().unwrap_or(self_type);
    let head_at = self_type_at + nominal.rfind("::").map_or(0, |at| at + 2);
    let response = query(remote, root, file, text, head_at, TYPE_DEF).await?;
    let targets = location_identities(&response)?;
    if targets.len() == 1 {
        return Some(OwnerIdentity {
            head: targets[0].clone(),
            targets,
        });
    }
    let head_def = query(remote, root, file, text, head_at, DEF).await?;
    let head = location_identity(&head_def)?;
    if !targets.contains(&head) {
        return None;
    }
    let args_offset = self_type_at + nominal.len();
    let args = Source::new(&self_type[nominal.len()..]).ok()?;
    let mut arg_defs = Vec::new();
    for (i, token) in args.tokens.iter().enumerate() {
        if token.kind == Kind::Ident
            && !args.is(i + 1, "::")
            && !args.is(i + 1, "!")
            && !args.is(i + 1, "=")
        {
            let at = args_offset + token.start;
            let def = query(remote, root, file, text, at, DEF).await?;
            if let Some(id) = location_identity(&def) {
                arg_defs.push(id);
            }
        }
    }
    if arg_defs.is_empty()
        || !targets.iter().all(|t| *t == head || arg_defs.contains(t))
        || !arg_defs.iter().all(|a| targets.contains(a))
        || (!arg_defs.contains(&head) && targets.iter().filter(|&t| *t == head).count() != 1)
    {
        return None;
    }
    Some(OwnerIdentity { head, targets })
}

fn location_identities(response: &Value) -> Option<Vec<Value>> {
    match response {
        Value::Array(locations) if !locations.is_empty() => {
            locations.iter().map(location_identity).collect()
        }
        Value::Object(_) => Some(vec![location_identity(response)?]),
        _ => None,
    }
}

fn location_identity(response: &Value) -> Option<Value> {
    let location = match response {
        Value::Array(locations) if locations.len() == 1 => &locations[0],
        Value::Object(_) => response,
        _ => return None,
    };
    // Both LSP Location and LocationLink are permitted. Never compare just the URI.
    let uri = location
        .get("uri")
        .or_else(|| location.get("targetUri"))?
        .as_str()?;
    let start = location
        .pointer("/range/start")
        .or_else(|| location.pointer("/targetSelectionRange/start"))?;
    Some(json!([
        uri,
        start.get("line")?.as_u64()?,
        start.get("character")?.as_u64()?
    ]))
}

pub(super) struct ReceiverProof<'a> {
    pub remote: SocketAddr,
    pub root: &'a Path,
    pub owner: &'a str,
    pub identity: Option<OwnerIdentity>,
    pub declaration: (&'a Path, usize),
}

pub(super) type FileEdits = BTreeMap<PathBuf, Vec<(usize, usize, String)>>;

pub(super) struct CallEdit<'a> {
    pub file: &'a Path,
    pub from: usize,
    pub len: usize,
    pub replacement: &'a str,
    pub method_offset: usize,
}

fn shifted(at: usize, edits: &[(usize, usize, String)]) -> usize {
    edits
        .iter()
        .filter(|(from, len, _)| from + len <= at)
        .fold(at as isize, |at, (_, len, replacement)| {
            at + replacement.len() as isize - *len as isize
        }) as usize
}

pub(super) fn render(text: &str, edits: &[(usize, usize, String)]) -> String {
    let mut out = text.to_string();
    let mut edits = edits.iter().collect::<Vec<_>>();
    edits.sort_by_key(|(at, _, _)| *at);
    for (at, len, replacement) in edits.into_iter().rev() {
        out.replace_range(*at..at + len, replacement);
    }
    out
}

impl ReceiverProof<'_> {
    /// Resolve the dot call with the converted declaration in an unsaved overlay.
    /// Even an exact nominal receiver can select a trait method before autoref.
    pub(super) async fn proves_dispatch(
        &self,
        texts: &BTreeMap<PathBuf, String>,
        edits: &FileEdits,
        call: &CallEdit<'_>,
    ) -> bool {
        // A whole-call replacement contains the original nested expressions.
        // Keep an overlapping call qualified rather than render incompatible
        // original-coordinate edits (in either analyzer reference order).
        if edits.get(call.file).is_some_and(|edits| {
            edits
                .iter()
                .any(|(from, len, _)| call.from < from + len && *from < call.from + call.len)
        }) {
            return false;
        }
        let mut proposed = edits.clone();
        proposed.entry(call.file.to_path_buf()).or_default().push((
            call.from,
            call.len,
            call.replacement.to_string(),
        ));
        let call_at = shifted(call.from, &proposed[call.file]) + call.method_offset;
        let declaration_at = shifted(self.declaration.1, &proposed[self.declaration.0]);
        let overlays = proposed
            .iter()
            .map(|(file, edits)| Some((file.clone(), render(texts.get(file)?, edits))))
            .collect::<Option<BTreeMap<_, _>>>();
        let Some(overlays) = overlays else {
            return false;
        };
        let Ok((dl, dc)) =
            crate::signature::position_at(&overlays[self.declaration.0], declaration_at)
        else {
            return false;
        };
        let Ok((cl, cc)) = crate::signature::position_at(&overlays[call.file], call_at) else {
            return false;
        };
        let Ok(mut session) = crate::session::LspSession::open_for_validation(
            self.remote,
            self.root,
            Some(call.file),
        )
        .await
        else {
            return false;
        };
        for (file, text) in &overlays {
            if session.open_text(file, text).await.is_err() {
                session.close().await;
                return false;
            }
        }
        let result = session
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": session.uri_for(call.file).ok() },
                    "position": { "line": cl - 1, "character": cc - 1 }
                }),
            )
            .await;
        let expected = session
            .uri_for(self.declaration.0)
            .ok()
            .map(|uri| json!([uri, dl - 1, dc - 1]));
        session.close().await;
        result
            .ok()
            .as_ref()
            .and_then(location_identity)
            .is_some_and(|actual| Some(actual) == expected)
    }

    /// Bounded proof for a binding or a built-in slice/array element. A hover on the
    /// binding supplies its pre-coercion type; nominal identity and all generic arguments
    /// must agree with the impl. Custom Deref/Index, calls and other expressions stay UFCS.
    pub(super) async fn exact_receiver(
        &self,
        file: &Path,
        text: &str,
        argument_at: usize,
        argument: &str,
    ) -> bool {
        let Some(identity) = self.identity.as_ref() else {
            return false;
        };
        let argument = argument.trim();
        let place = argument
            .strip_prefix("&mut ")
            .or_else(|| argument.strip_prefix('&'))
            .unwrap_or(argument)
            .trim_start();
        let end = place.find(|c: char| !is_ident(c)).unwrap_or(place.len());
        let name = &place[..end];
        if name.is_empty() || !name.starts_with(|c: char| c == '_' || c.is_alphabetic()) {
            return false;
        }
        let suffix = place[end..].trim();
        let indexed = !suffix.is_empty();
        if indexed
            && !suffix
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .is_some_and(|s| {
                    !s.trim().is_empty() && s.trim().chars().all(|c| c.is_ascii_digit())
                })
        {
            return false;
        }
        let at = argument_at + argument.len() - place.len();
        let Some(hover) = query(self.remote, self.root, file, text, at, "textDocument/hover").await
        else {
            return false;
        };
        let Some(contents) = hover.pointer("/contents/value").and_then(Value::as_str) else {
            return false;
        };
        let ty = contents.lines().find_map(|line| {
            let line = line.trim().strip_prefix("let ").unwrap_or(line.trim());
            let line = line.strip_prefix("mut ").unwrap_or(line);
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            Some(rest.trim().trim_end_matches(';'))
        });
        let Some(mut ty) = ty else {
            return false;
        };
        // Strip built-in references for type identity; overlay lookup proves dispatch.
        while let Some(rest) = ty.strip_prefix('&') {
            ty = rest.trim_start();
            if ty.starts_with('\'') {
                let Some(end) = ty.find(char::is_whitespace) else {
                    return false;
                };
                ty = ty[end..].trim_start();
            }
            ty = ty.strip_prefix("mut ").unwrap_or(ty).trim_start();
        }
        if indexed {
            let Some(element) = ty.strip_prefix('[').and_then(|t| t.strip_suffix(']')) else {
                return false;
            };
            ty = element.split(';').next().unwrap_or(element).trim();
        }
        let Some(ty) = type_spelling(ty) else {
            return false;
        };
        if Some(ty) != type_spelling(self.owner) {
            return false;
        }
        let Some(response) = query(self.remote, self.root, file, text, at, TYPE_DEF).await else {
            return false;
        };
        if location_identities(&response).as_ref() != Some(&identity.targets) {
            return false;
        }
        identity.targets.len() == 1
            || self.binding_head(file, text, at, name).await.as_ref() == Some(&identity.head)
    }

    async fn binding_head(&self, file: &Path, text: &str, at: usize, name: &str) -> Option<Value> {
        use crate::fixture::builder::lexer::{Kind, Source};

        let def = query(self.remote, self.root, file, text, at, DEF).await?;
        let loc = location_identity(&def)?;
        let [Value::String(uri), Value::Number(line), Value::Number(col)] =
            loc.as_array()?.as_slice()
        else {
            return None;
        };
        let decl_path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
        let owned = (decl_path != file)
            .then(|| std::fs::read_to_string(&decl_path).ok())
            .flatten();
        let decl_text = owned.as_deref().unwrap_or(text);
        let (l, c) = (line.as_u64()? as u32 + 1, col.as_u64()? as u32 + 1);
        let base = crate::signature::offset_of(decl_text, l, c)? + name.len();
        let tokens = Source::new(decl_text.get(base..)?).ok()?;
        if !tokens.is(0, ":") && !tokens.is(0, "=") {
            return None;
        }
        let mut i = 1;
        while i < tokens.tokens.len()
            && (["&", "mut", "(", "["].iter().any(|s| tokens.is(i, s))
                || tokens.tokens[i].kind == Kind::Lifetime)
        {
            i += 1;
        }
        while i + 2 < tokens.tokens.len()
            && tokens.tokens[i].kind == Kind::Ident
            && tokens.is(i + 1, "::")
            && tokens.tokens[i + 2].kind == Kind::Ident
        {
            i += 2;
        }
        let tok = tokens.tokens.get(i).filter(|t| t.kind == Kind::Ident)?;
        let head_at = base + tok.start;
        let res = query(self.remote, self.root, &decl_path, decl_text, head_at, DEF).await?;
        location_identity(&res)
    }
}
