//! Safe deletion of a deliberately narrow Go function and receiver-method subset.
//!
//! gopls supplies the declaration range and a complete reference answer. The local source is
//! then checked against both answers, lexed without treating braces in comments or literals as
//! syntax, compiled in a remote shadow, and changed only after all evidence remains current.

use anyhow::{Context, Result, anyhow};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedFunction {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionRange {
    name: String,
    start: usize,
    end: usize,
    name_start: usize,
    name_end: usize,
    receiver: Option<Receiver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Receiver {
    type_name: String,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

pub async fn delete_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<DeletedFunction> {
    anyhow::ensure!(
        file.extension().is_some_and(|extension| extension == "go"),
        "{} is not a Go source file; nothing was written",
        file.display()
    );
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve checkout {}", root.display()))?;
    refuse_linked_source_path(root, &canonical_root, file)?;
    let canonical_file = regular_unlinked_inside(&canonical_root, file)?;
    let module = go_module_root(&canonical_root, &canonical_file)?;
    refuse_linked_sources(&module)?;

    let original = std::fs::read_to_string(&canonical_file).with_context(|| {
        format!(
            "cannot read {}; nothing was written",
            canonical_file.display()
        )
    })?;
    anyhow::ensure!(
        !is_generated(&original),
        "{} is generated Go source; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    refuse_source_directives(&original)?;
    anyhow::ensure!(
        line > 0 && col > 0,
        "{}:{line}:{col} is not on a declaration name; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    let requested = offset_at(&original, line - 1, col - 1).with_context(|| {
        format!(
            "{}:{line}:{col} is not a valid UTF-16 source position; nothing was written",
            display(&canonical_root, &canonical_file)
        )
    })?;

    let uri = url::Url::from_file_path(&canonical_file)
        .map_err(|_| anyhow!("invalid Go source path {}", canonical_file.display()))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        &canonical_root,
        &canonical_file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
    .context("gopls could not describe the Go declarations; nothing was written")?;
    let function = function_at(&canonical_file, &original, requested, &symbols)
        .context("Go safe delete refused; nothing was written")?;

    let receiver_sources = if let Some(receiver) = &function.receiver {
        let sources = receiver_source_evidence(&canonical_file, &original, receiver)
            .context("Go receiver-method safe delete refused; nothing was written")?;
        crate::signature_go::receiver_interface_evidence(
            remote,
            &canonical_root,
            &canonical_file,
            &original,
            &function.name,
            function.name_start,
        )
        .await
        .context(
            "Go receiver-method safe delete requires empty gopls implementation evidence; nothing was written",
        )?;
        if let Some(path) = crate::signature_go::package_interface_method_file(
            &canonical_file,
            &function.name,
        )
        .context(
            "Go receiver-method safe delete could not inspect local interface obligations; nothing was written",
        )? {
            anyhow::bail!(
                "Go receiver-method safe delete refused; {} has a local interface obligation in {}; nothing was written",
                function.name,
                display(&canonical_root, &path)
            );
        }
        Some(sources)
    } else {
        None
    };

    anyhow::ensure!(
        current_text(&canonical_file, &original)?,
        "{} changed while its declaration was inspected; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    if let Some(expected) = &receiver_sources {
        anyhow::ensure!(
            package_sources(&canonical_file, &original)? == *expected,
            "the declaring Go package changed while receiver-method evidence was compiled; nothing was written"
        );
    }
    reference_evidence(
        remote,
        &canonical_root,
        &canonical_file,
        &original,
        &function,
    )
    .await
    .context("Go safe delete refused; nothing was written")?;

    let mut proposed = original.clone();
    proposed.replace_range(function.start..function.end, "");
    let verdict = crate::verify::compile_go_shadow(
        remote,
        &canonical_root,
        &canonical_file,
        &[(canonical_file.clone(), proposed.clone())],
    )
    .await
    .context("Go safe delete compiler evidence is unavailable; nothing was written")?;
    anyhow::ensure!(
        verdict.passed,
        "deleting {} does not compile under the active Go build flags; nothing was written:\n{}",
        function.name,
        verdict.output.trim()
    );
    anyhow::ensure!(
        current_text(&canonical_file, &original)?,
        "{} changed while the deletion was compiled; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    if let Some(expected) = &receiver_sources {
        anyhow::ensure!(
            package_sources(&canonical_file, &original)? == *expected,
            "the declaring Go package changed while the receiver-method deletion was compiled; nothing was written"
        );
    }

    let mut files = BTreeMap::new();
    files.insert(canonical_file.clone(), proposed);
    let touched = crate::refactor::apply_workspace_edit(
        &canonical_root,
        &crate::signature::whole_file_edit(&files),
    )
    .context("the verified Go deletion could not be applied")?;
    anyhow::ensure!(
        touched.len() == 1,
        "the verified Go deletion updated {} paths instead of one",
        touched.len()
    );
    Ok(DeletedFunction {
        name: function.name,
        path: canonical_file,
    })
}

fn function_at(
    file: &Path,
    text: &str,
    requested: usize,
    answer: &serde_json::Value,
) -> Result<FunctionRange> {
    let symbols = answer.as_array().with_context(|| {
        format!("textDocument/documentSymbol returned no usable list: {answer}")
    })?;
    let mut selected = Vec::new();
    collect_selected(file, text, requested, symbols, &mut selected)?;
    anyhow::ensure!(
        selected.len() == 1,
        "the position is on {} declaration names instead of exactly one",
        selected.len()
    );
    let (name, kind, range, selection) = selected.pop().expect("one selected symbol");
    anyhow::ensure!(
        matches!(kind, 6 | 12),
        "the position names a non-function declaration, not an ordinary function or receiver method"
    );
    let function = parse_function(text, &name, range, selection)?;
    anyhow::ensure!(
        matches!((kind, function.receiver.is_some()), (6, true) | (12, false)),
        "the analyzer's declaration kind does not match the current function declaration"
    );
    Ok(function)
}

fn collect_selected(
    file: &Path,
    text: &str,
    requested: usize,
    symbols: &[serde_json::Value],
    selected: &mut Vec<(String, u64, Span, Span)>,
) -> Result<()> {
    for symbol in symbols {
        let (Some(name), Some(kind)) = (
            symbol.get("name").and_then(serde_json::Value::as_str),
            symbol.get("kind").and_then(serde_json::Value::as_u64),
        ) else {
            anyhow::bail!("a document symbol is malformed: {symbol}");
        };
        anyhow::ensure!(
            !name.is_empty() && (1..=26).contains(&kind),
            "a document symbol is malformed: {symbol}"
        );

        let location_range = symbol.pointer("/location/range");
        if let Some(location) = symbol.get("location") {
            let uri = location
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .context("a symbol location has no URI")?;
            let uri = url::Url::parse(uri).context("a symbol location has an invalid URI")?;
            anyhow::ensure!(
                uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
                "a symbol location is not a plain local file URI"
            );
            let reported = uri
                .to_file_path()
                .map_err(|_| anyhow!("a symbol location is not a local file URI"))?;
            anyhow::ensure!(
                same_file(file, &reported),
                "a document symbol points at another file"
            );
        }

        let range = symbol
            .get("range")
            .or(location_range)
            .context("a document symbol has no declaration range")
            .and_then(|value| lsp_span(text, value))
            .context("a document symbol has a malformed range")?;
        let selection = match symbol.get("selectionRange") {
            Some(value) => Some(
                lsp_span(text, value).context("a document symbol has a malformed name range")?,
            ),
            None => match range {
                range if range.start <= requested && requested < range.end => Some(
                    infer_name_span(text, name, range)
                        .context("a document symbol has no usable name range")?,
                ),
                _ => None,
            },
        };
        if let Some(selection) = selection {
            anyhow::ensure!(
                range.start <= selection.start && selection.end <= range.end,
                "a document symbol's name is outside its declaration range"
            );
            anyhow::ensure!(
                selection.start < selection.end,
                "a document symbol has an empty name range"
            );
            if selection.start <= requested && requested < selection.end {
                selected.push((name.to_string(), kind, range, selection));
            }
        }
        match symbol.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => {
                collect_selected(file, text, requested, children, selected)?
            }
            Some(_) => anyhow::bail!("a document symbol has malformed children"),
        }
    }
    Ok(())
}

fn infer_name_span(text: &str, symbol_name: &str, range: Span) -> Result<Span> {
    anyhow::ensure!(
        range.start < range.end && range.end <= text.len(),
        "the declaration range is empty or outside the source"
    );
    anyhow::ensure!(
        text.get(range.start..range.start + 4) == Some("func")
            && token_boundary(text.as_bytes(), range.start, 4),
        "the declaration range does not begin at the func keyword"
    );
    let mut cursor = skip_trivia(text, range.start + 4)?;
    if text.as_bytes().get(cursor) == Some(&b'(') {
        cursor = skip_trivia(text, matching(text, cursor)? + 1)?;
    }
    let end = ascii_identifier_end(text, cursor)
        .context("the declaration range has no ordinary ASCII function name")?;
    let source_name = &text[cursor..end];
    anyhow::ensure!(
        symbol_matches(symbol_name, source_name),
        "the analyzer's symbol name {symbol_name} does not match {source_name}"
    );
    Ok(Span { start: cursor, end })
}

fn parse_function(
    text: &str,
    symbol_name: &str,
    analyzer_range: Span,
    selection: Span,
) -> Result<FunctionRange> {
    let source_name = text
        .get(selection.start..selection.end)
        .context("the declaration name range is stale")?;
    anyhow::ensure!(
        symbol_matches(symbol_name, source_name),
        "the analyzer's symbol name {symbol_name} does not match {source_name}"
    );
    anyhow::ensure!(
        ascii_unexported_name(source_name),
        "{} is not a supported unexported ASCII Go function name",
        source_name
    );
    anyhow::ensure!(
        !matches!(source_name, "main" | "init"),
        "Go entry point {} cannot be safely deleted",
        source_name
    );

    let start = function_start_for_name(text, selection.start)?;
    ensure_top_level(text, start)?;
    refuse_attached_directives(text, start)?;
    let mut cursor = skip_trivia(text, start + 4)?;
    let receiver = if text.as_bytes().get(cursor) == Some(&b'(') {
        let close = matching(text, cursor)?;
        let receiver = parse_receiver(&text[cursor + 1..close])?;
        cursor = skip_trivia(text, close + 1)?;
        Some(receiver)
    } else {
        None
    };
    let name_start = cursor;
    let name_end = ascii_identifier_end(text, name_start)
        .context("the function declaration has no ordinary ASCII name")?;
    anyhow::ensure!(
        &text[name_start..name_end] == source_name
            && selection.start == name_start
            && selection.end == name_end,
        "the analyzer's name range does not match the current declaration"
    );
    cursor = skip_trivia(text, name_end)?;
    anyhow::ensure!(
        text.as_bytes().get(cursor) != Some(&b'['),
        "generic Go functions are not supported"
    );
    anyhow::ensure!(
        text.as_bytes().get(cursor) == Some(&b'('),
        "the function has no parameter list"
    );
    let parameters_close = matching(text, cursor)?;
    cursor = parameters_close + 1;
    let mut anonymous_composite = false;
    let body_close = loop {
        cursor = skip_trivia(text, cursor)?;
        let byte = *text
            .as_bytes()
            .get(cursor)
            .context("the declaration has no complete body")?;
        match byte {
            b'\n' | b';' => anyhow::bail!("the declaration has no body here"),
            b'(' | b'[' => cursor = matching(text, cursor)? + 1,
            b'{' => {
                let close = matching(text, cursor)?;
                if anonymous_composite {
                    cursor = close + 1;
                    anonymous_composite = false;
                } else {
                    break close;
                }
            }
            b'"' | b'\'' | b'\x60' => {
                cursor = opaque_end(text, cursor)?
                    .context("unterminated literal in the function signature")?
            }
            first if first.is_ascii_alphabetic() || first == b'_' => {
                let end = ascii_identifier_end(text, cursor)
                    .context("an identifier in the function signature is malformed")?;
                anonymous_composite = matches!(&text[cursor..end], "struct" | "interface");
                cursor = end;
            }
            _ => {
                anonymous_composite = false;
                cursor += 1;
            }
        }
    };
    let end = body_close + 1;
    anyhow::ensure!(
        analyzer_range.start == start
            && trim_ascii_end(text, analyzer_range.start, analyzer_range.end) == end,
        "the analyzer's declaration range does not match the current function body"
    );
    Ok(FunctionRange {
        name: source_name.to_string(),
        start,
        end,
        name_start,
        name_end,
        receiver,
    })
}

fn parse_receiver(source: &str) -> Result<Receiver> {
    anyhow::ensure!(
        !source.contains("//") && !source.contains("/*"),
        "receiver comments make the declaration ambiguous"
    );
    let pieces: Vec<&str> = source.split_ascii_whitespace().collect();
    anyhow::ensure!(
        pieces.len() == 2 && ascii_identifier(pieces[0]) && pieces[0] != "_",
        "a receiver method must bind exactly one ordinary named receiver"
    );
    let named = pieces[1].strip_prefix('*').unwrap_or(pieces[1]);
    anyhow::ensure!(
        ascii_identifier(named),
        "receiver type {} is generic, qualified, aliased, or otherwise ambiguous",
        pieces[1]
    );
    Ok(Receiver {
        type_name: named.to_string(),
    })
}

fn receiver_source_evidence(
    file: &Path,
    text: &str,
    receiver: &Receiver,
) -> Result<BTreeMap<PathBuf, String>> {
    let sources = package_sources(file, text)?;
    let declarations: Vec<(bool, bool)> = sources
        .values()
        .flat_map(|source| type_declarations(source, &receiver.type_name))
        .collect();
    anyhow::ensure!(
        declarations.len() == 1,
        "receiver type {} has {} package declarations instead of exactly one",
        receiver.type_name,
        declarations.len()
    );
    let (alias, generic) = declarations[0];
    anyhow::ensure!(!alias, "receiver type {} is an alias", receiver.type_name);
    anyhow::ensure!(
        !generic,
        "receiver type {} is generic or parameterized",
        receiver.type_name
    );
    let receiver_names = receiver_type_names(&sources, &receiver.type_name)?;
    for (path, source) in &sources {
        anyhow::ensure!(
            !embeds_receiver(source, &receiver_names)?,
            "receiver type {} is embedded or promoted in {}",
            receiver.type_name,
            path.display()
        );
    }
    Ok(sources)
}

fn package_sources(file: &Path, declaration_text: &str) -> Result<BTreeMap<PathBuf, String>> {
    let directory = file
        .parent()
        .with_context(|| format!("{} has no containing package directory", file.display()))?;
    let package = go_package_name(declaration_text)
        .with_context(|| format!("cannot identify the Go package in {}", file.display()))?;
    let mut sources = BTreeMap::new();
    for entry in std::fs::read_dir(directory).with_context(|| {
        format!(
            "cannot inspect Go package directory {}",
            directory.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot inspect an entry in Go package directory {}",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "go") {
            continue;
        }
        let kind = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "linked Go source {} cannot be inspected",
            path.display()
        );
        if !kind.is_file() {
            continue;
        }
        let source = if same_file(file, &path) {
            declaration_text.to_string()
        } else {
            std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?
        };
        if go_package_name(&source) == Some(package) {
            sources.insert(path, source);
        }
    }
    anyhow::ensure!(
        sources.keys().any(|path| same_file(path, file)),
        "the declaring Go source disappeared from its package"
    );
    Ok(sources)
}

fn go_package_name(text: &str) -> Option<&str> {
    let at = skip_trivia(text, 0).ok()?;
    if !text[at..].starts_with("package") || !token_boundary(text.as_bytes(), at, 7) {
        return None;
    }
    let start = skip_trivia(text, at + 7).ok()?;
    let end = ascii_identifier_end(text, start)?;
    Some(&text[start..end])
}

/// Returns `(alias, generic)` for every package-level declaration of `wanted`.
fn type_declarations(text: &str, wanted: &str) -> Vec<(bool, bool)> {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens, mut cursor) = (0usize, 0usize, 0usize, 0usize);
    let mut found = Vec::new();
    while cursor < bytes.len() {
        if let Ok(Some(end)) = opaque_end(text, cursor) {
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b't' if braces == 0
                && brackets == 0
                && parens == 0
                && bytes[cursor..].starts_with(b"type")
                && token_boundary(bytes, cursor, 4) =>
            {
                let Ok(start) = skip_trivia(text, cursor + 4) else {
                    return found;
                };
                if bytes.get(start) == Some(&b'(') {
                    let Ok(close) = matching(text, start) else {
                        return found;
                    };
                    grouped_type_declarations(text, start, close, wanted, &mut found);
                    cursor = close + 1;
                    continue;
                }
                if let Some(end) = ascii_identifier_end(text, start) {
                    if &text[start..end] == wanted {
                        found.push(declaration_shape(text, end));
                    }
                    cursor = end;
                    continue;
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    found
}

fn grouped_type_declarations(
    text: &str,
    open: usize,
    close: usize,
    wanted: &str,
    found: &mut Vec<(bool, bool)>,
) {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens) = (0usize, 0usize, 0usize);
    let (mut cursor, mut spec_start) = (open + 1, true);
    while cursor < close {
        if let Ok(Some(end)) = opaque_end(text, cursor) {
            if text[cursor..end].contains('\n') && braces == 0 && brackets == 0 && parens == 0 {
                spec_start = true;
            }
            cursor = end;
            continue;
        }
        if spec_start && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if spec_start
            && braces == 0
            && brackets == 0
            && parens == 0
            && let Some(end) = ascii_identifier_end(text, cursor)
        {
            if &text[cursor..end] == wanted {
                found.push(declaration_shape(text, end));
            }
            spec_start = false;
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b';' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            b'\n' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            _ => {}
        }
        cursor += 1;
    }
}

fn declaration_shape(text: &str, name_end: usize) -> (bool, bool) {
    let Ok(after) = skip_trivia(text, name_end) else {
        return (false, true);
    };
    if text.as_bytes().get(after) == Some(&b'=') {
        return (true, false);
    }
    if text.as_bytes().get(after) != Some(&b'[') {
        return (false, false);
    }
    let Ok(close) = matching(text, after) else {
        return (false, true);
    };
    let Ok(tail) = skip_trivia(text, close + 1) else {
        return (false, true);
    };
    (
        text.as_bytes().get(tail) == Some(&b'='),
        bracket_declares_type_parameters(text, after, close),
    )
}

fn bracket_declares_type_parameters(text: &str, open: usize, close: usize) -> bool {
    let Ok(start) = skip_trivia(text, open + 1) else {
        return true;
    };
    if start == close || text[start..close].starts_with("...") {
        return false;
    }
    let Some(name_end) = ascii_identifier_end(text, start) else {
        return false;
    };
    let Ok(after) = skip_trivia(text, name_end) else {
        return true;
    };
    if after == close {
        return false;
    }
    match text.as_bytes()[after] {
        b'.' | b'(' | b'+' | b'-' | b'/' | b'%' | b'&' | b'|' | b'^' | b'<' | b'>' => false,
        b',' | b'~' | b'[' | b'*' => true,
        byte if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80 => true,
        _ => true,
    }
}

fn receiver_type_names(
    sources: &BTreeMap<PathBuf, String>,
    receiver: &str,
) -> Result<BTreeSet<String>> {
    let aliases = sources
        .values()
        .map(|source| type_aliases(source))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut names = BTreeSet::from([receiver.to_string()]);
    loop {
        let mut changed = false;
        for (alias, target) in &aliases {
            if names.contains(target) {
                changed |= names.insert(alias.clone());
            }
        }
        if !changed {
            return Ok(names);
        }
    }
}

fn type_aliases(text: &str) -> Result<Vec<(String, String)>> {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens, mut cursor) = (0usize, 0usize, 0usize, 0usize);
    let mut aliases = Vec::new();
    while cursor < bytes.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b't' if braces == 0
                && brackets == 0
                && parens == 0
                && bytes[cursor..].starts_with(b"type")
                && token_boundary(bytes, cursor, 4) =>
            {
                let start = skip_trivia(text, cursor + 4)?;
                if bytes.get(start) == Some(&b'(') {
                    let close = matching(text, start)?;
                    grouped_type_aliases(text, start, close, &mut aliases)?;
                    cursor = close + 1;
                    continue;
                }
                if let Some(end) = ascii_identifier_end(text, start) {
                    if let Some(target) = type_alias_target(text, end)? {
                        aliases.push((text[start..end].to_string(), target));
                    }
                    cursor = end;
                    continue;
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(aliases)
}

fn grouped_type_aliases(
    text: &str,
    open: usize,
    close: usize,
    aliases: &mut Vec<(String, String)>,
) -> Result<()> {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens) = (0usize, 0usize, 0usize);
    let (mut cursor, mut spec_start) = (open + 1, true);
    while cursor < close {
        if let Some(end) = opaque_end(text, cursor)? {
            if text[cursor..end].contains('\n') && braces == 0 && brackets == 0 && parens == 0 {
                spec_start = true;
            }
            cursor = end;
            continue;
        }
        if spec_start && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if spec_start
            && braces == 0
            && brackets == 0
            && parens == 0
            && let Some(end) = ascii_identifier_end(text, cursor)
        {
            if let Some(target) = type_alias_target(text, end)? {
                aliases.push((text[cursor..end].to_string(), target));
            }
            spec_start = false;
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b';' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            b'\n' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            _ => {}
        }
        cursor += 1;
    }
    Ok(())
}

fn type_alias_target(text: &str, name_end: usize) -> Result<Option<String>> {
    let mut cursor = skip_trivia(text, name_end)?;
    if text.as_bytes().get(cursor) == Some(&b'[') {
        cursor = skip_trivia(text, matching(text, cursor)? + 1)?;
    }
    if text.as_bytes().get(cursor) != Some(&b'=') {
        return Ok(None);
    }
    cursor = skip_trivia(text, cursor + 1)?;
    if text.as_bytes().get(cursor) == Some(&b'*') {
        cursor = skip_trivia(text, cursor + 1)?;
    }
    let Some(end) = ascii_identifier_end(text, cursor) else {
        return Ok(None);
    };
    Ok(Some(text[cursor..end].to_string()))
}

fn embeds_receiver(text: &str, receiver_names: &BTreeSet<String>) -> Result<bool> {
    let bytes = text.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        if bytes[cursor..].starts_with(b"struct") && token_boundary(bytes, cursor, 6) {
            let open = skip_trivia(text, cursor + 6)?;
            if bytes.get(open) == Some(&b'{') {
                let close = matching(text, open)?;
                if struct_body_embeds(&text[open + 1..close], receiver_names)? {
                    return Ok(true);
                }
                cursor = open + 1;
                continue;
            }
        }
        cursor += 1;
    }
    Ok(false)
}

fn struct_body_embeds(body: &str, receiver_names: &BTreeSet<String>) -> Result<bool> {
    let bytes = body.as_bytes();
    let (mut braces, mut brackets, mut parens, mut start, mut cursor) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    while cursor <= bytes.len() {
        if cursor == bytes.len() {
            return field_embeds(&body[start..cursor], receiver_names);
        }
        if let Some(end) = opaque_end(body, cursor)? {
            if bytes[cursor] == b'/'
                && body[cursor..end].contains('\n')
                && braces == 0
                && brackets == 0
                && parens == 0
            {
                if field_embeds(&body[start..cursor], receiver_names)? {
                    return Ok(true);
                }
                start = end;
            }
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b';' | b'\n' if braces == 0 && brackets == 0 && parens == 0 => {
                if field_embeds(&body[start..cursor], receiver_names)? {
                    return Ok(true);
                }
                start = cursor + 1;
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(false)
}

fn field_embeds(field: &str, receiver_names: &BTreeSet<String>) -> Result<bool> {
    let mut cursor = skip_trivia(field, 0)?;
    if field.as_bytes().get(cursor) == Some(&b'*') {
        cursor += 1;
    }
    let Some(end) = ascii_identifier_end(field, cursor) else {
        return Ok(false);
    };
    if !receiver_names.contains(&field[cursor..end]) {
        return Ok(false);
    }
    cursor = skip_trivia(field, end)?;
    if cursor == field.len() {
        return Ok(true);
    }
    if matches!(field.as_bytes().get(cursor), Some(b'"' | b'\'' | b'\x60')) {
        let tag_end = opaque_end(field, cursor)?.context("an embedded field tag is malformed")?;
        return Ok(skip_trivia(field, tag_end)? == field.len());
    }
    Ok(false)
}

fn function_start_for_name(text: &str, name_start: usize) -> Result<usize> {
    let mut stack = Vec::new();
    let mut cursor = 0usize;
    while cursor < name_start {
        if let Some(next) = opaque_end(text, cursor)? {
            cursor = next;
            continue;
        }
        match text.as_bytes()[cursor] {
            open @ (b'(' | b'[' | b'{') => stack.push(open),
            close @ (b')' | b']' | b'}') => {
                let opened = stack
                    .pop()
                    .context("unmatched closing delimiter before the function")?;
                anyhow::ensure!(
                    pair(opened, close),
                    "mismatched delimiter before the function"
                );
            }
            b'f' if stack.is_empty()
                && text.as_bytes()[cursor..].starts_with(b"func")
                && token_boundary(text.as_bytes(), cursor, 4) =>
            {
                let mut candidate = skip_trivia(text, cursor + 4)?;
                if text.as_bytes().get(candidate) == Some(&b'(') {
                    candidate = skip_trivia(text, matching(text, candidate)? + 1)?;
                }
                if candidate == name_start {
                    return Ok(cursor);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    anyhow::bail!("the analyzer's name is not on a top-level Go function declaration")
}

fn symbol_matches(symbol: &str, source: &str) -> bool {
    symbol == source
        || symbol.strip_suffix("()") == Some(source)
        || symbol.rsplit('.').next() == Some(source)
}

fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

async fn reference_evidence(
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

fn lsp_span(text: &str, range: &serde_json::Value) -> Result<Span> {
    let number = |end: &str, field: &str| {
        range
            .pointer(&format!("/{end}/{field}"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value < u32::MAX)
    };
    let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) = (
        number("start", "line"),
        number("start", "character"),
        number("end", "line"),
        number("end", "character"),
    ) else {
        anyhow::bail!("missing or oversized line/character");
    };
    let start = offset_at(text, start_line, start_col)
        .context("the range start is not a valid UTF-16 source position")?;
    let end = offset_at(text, end_line, end_col)
        .context("the range end is not a valid UTF-16 source position")?;
    anyhow::ensure!(start <= end, "the range runs backwards");
    Ok(Span { start, end })
}

fn offset_at(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        start += text[start..].find('\n')? + 1;
    }
    let rest = &text[start..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let end = line_end - usize::from(line_end < rest.len() && rest[..line_end].ends_with('\r'));
    let mut units = 0u32;
    for (byte, character) in rest[..end].char_indices() {
        if units >= col {
            return (units == col).then_some(start + byte);
        }
        units = units.checked_add(character.len_utf16() as u32)?;
    }
    (units == col).then_some(start + end)
}

fn line_col_utf16(text: &str, offset: usize) -> Result<(u32, u32)> {
    anyhow::ensure!(
        offset <= text.len() && text.is_char_boundary(offset),
        "a source offset is not a UTF-8 boundary"
    );
    let before = &text[..offset];
    let line = u32::try_from(before.matches('\n').count())?;
    let column = u32::try_from(
        before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .encode_utf16()
            .count(),
    )?;
    Ok((line, column))
}

fn ensure_top_level(text: &str, end: usize) -> Result<()> {
    let mut stack = Vec::new();
    let mut cursor = 0usize;
    while cursor < end {
        if let Some(next) = opaque_end(text, cursor)? {
            anyhow::ensure!(
                next <= end,
                "a comment or literal overlaps the declaration range"
            );
            cursor = next;
            continue;
        }
        match text.as_bytes()[cursor] {
            open @ (b'(' | b'[' | b'{') => stack.push(open),
            close @ (b')' | b']' | b'}') => {
                let open = stack
                    .pop()
                    .context("unmatched closing delimiter before the function")?;
                anyhow::ensure!(
                    pair(open, close),
                    "mismatched delimiter before the function"
                );
            }
            _ => {}
        }
        cursor += 1;
    }
    anyhow::ensure!(
        stack.is_empty(),
        "the function is nested in another declaration or expression"
    );
    Ok(())
}

fn matching(text: &str, open: usize) -> Result<usize> {
    let first = *text
        .as_bytes()
        .get(open)
        .context("a delimiter is outside the source")?;
    anyhow::ensure!(
        matches!(first, b'(' | b'[' | b'{'),
        "expected an opening delimiter"
    );
    let mut stack = vec![first];
    let mut cursor = open + 1;
    while cursor < text.len() {
        if let Some(next) = opaque_end(text, cursor)? {
            cursor = next;
            continue;
        }
        match text.as_bytes()[cursor] {
            next @ (b'(' | b'[' | b'{') => stack.push(next),
            close @ (b')' | b']' | b'}') => {
                let opened = stack
                    .pop()
                    .context("an unmatched delimiter closes the declaration")?;
                anyhow::ensure!(
                    pair(opened, close),
                    "a delimiter is mismatched in the declaration"
                );
                if stack.is_empty() {
                    return Ok(cursor);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    anyhow::bail!("an opening delimiter in the declaration is not closed")
}

fn pair(open: u8, close: u8) -> bool {
    matches!((open, close), (b'(', b')') | (b'[', b']') | (b'{', b'}'))
}

fn opaque_end(text: &str, start: usize) -> Result<Option<usize>> {
    let bytes = text.as_bytes();
    let Some(&first) = bytes.get(start) else {
        return Ok(None);
    };
    let end = match first {
        quote @ (b'"' | b'\'') => {
            let mut cursor = start + 1;
            loop {
                let byte = *bytes
                    .get(cursor)
                    .context("a quoted literal is not terminated")?;
                match byte {
                    b'\\' => {
                        cursor = cursor
                            .checked_add(2)
                            .context("a quoted literal escape is truncated")?;
                    }
                    b'\n' => anyhow::bail!("a quoted literal crosses a line without closing"),
                    value if value == quote => break cursor + 1,
                    _ => cursor += 1,
                }
            }
        }
        b'\x60' => bytes[start + 1..]
            .iter()
            .position(|byte| *byte == b'\x60')
            .map(|offset| start + offset + 2)
            .context("a raw string literal is not terminated")?,
        b'/' if bytes.get(start + 1) == Some(&b'/') => bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| start + offset),
        b'/' if bytes.get(start + 1) == Some(&b'*') => bytes[start + 2..]
            .windows(2)
            .position(|window| window == b"*/")
            .map(|offset| start + offset + 4)
            .context("a block comment is not terminated")?,
        _ => return Ok(None),
    };
    Ok(Some(end))
}

fn skip_trivia(text: &str, mut cursor: usize) -> Result<usize> {
    loop {
        while text
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor += 1;
        }
        match opaque_end(text, cursor)? {
            Some(end) if text.as_bytes()[cursor] == b'/' => cursor = end,
            _ => return Ok(cursor),
        }
    }
}

fn ascii_identifier_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut end = start + 1;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        end += 1;
    }
    Some(end)
}

fn ascii_unexported_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

fn ascii_identifier(name: &str) -> bool {
    let Some(end) = ascii_identifier_end(name, 0) else {
        return false;
    };
    end == name.len()
        && name != "_"
        && !matches!(
            name,
            "break"
                | "default"
                | "func"
                | "interface"
                | "select"
                | "case"
                | "defer"
                | "go"
                | "map"
                | "struct"
                | "chan"
                | "else"
                | "goto"
                | "package"
                | "switch"
                | "const"
                | "fallthrough"
                | "if"
                | "range"
                | "type"
                | "continue"
                | "for"
                | "import"
                | "return"
                | "var"
        )
}

fn token_boundary(bytes: &[u8], start: usize, len: usize) -> bool {
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80;
    !start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| identifier(*byte))
        && !bytes.get(start + len).is_some_and(|byte| identifier(*byte))
}

fn trim_ascii_end(text: &str, start: usize, mut end: usize) -> usize {
    while end > start && text.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

fn refuse_attached_directives(text: &str, start: usize) -> Result<()> {
    let line_start = text[..start].rfind('\n').map_or(0, |newline| newline + 1);
    anyhow::ensure!(
        skip_trivia(text, line_start)? == start,
        "the declaration range begins after non-comment source on the same line"
    );
    let mut lines = text[..line_start].lines().rev();
    for line in &mut lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if !trimmed.starts_with("//") {
            break;
        }
        anyhow::ensure!(
            !trimmed.starts_with("//go:") && !trimmed.starts_with("//export "),
            "a Go compiler or cgo directive is attached to the function"
        );
    }
    Ok(())
}

fn is_generated(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut cursor = usize::from(text.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let rest = &bytes[cursor..];
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| cursor + offset);
            let line = text[cursor..end].trim_end_matches('\r');
            if let Some(marker) = line.strip_prefix("// Code generated ")
                && marker.ends_with(" DO NOT EDIT.")
            {
                return true;
            }
            cursor = end;
            continue;
        }
        if rest.starts_with(b"/*") {
            let Some(end) = rest[2..].windows(2).position(|window| window == b"*/") else {
                return false;
            };
            cursor += end + 4;
            continue;
        }
        return false;
    }
    false
}

fn refuse_source_directives(text: &str) -> Result<()> {
    let mut cursor = 0usize;
    while cursor < text.len() {
        let rest = &text.as_bytes()[cursor..];
        anyhow::ensure!(
            !rest.starts_with(b"//line ") && !rest.starts_with(b"/*line "),
            "Go source line directives are not supported; nothing was written"
        );
        cursor = opaque_end(text, cursor)?.unwrap_or(cursor + 1);
    }
    Ok(())
}

fn current_text(path: &Path, expected: &str) -> Result<bool> {
    Ok(std::fs::read_to_string(path)
        .with_context(|| format!("cannot reread {}", path.display()))?
        == expected)
}

fn refuse_linked_source_path(root: &Path, canonical_root: &Path, source: &Path) -> Result<()> {
    let relative = source
        .strip_prefix(root)
        .or_else(|_| source.strip_prefix(canonical_root))
        .with_context(|| {
            format!(
                "{} is outside the checkout {}; nothing was written",
                source.display(),
                root.display()
            )
        })?;
    let mut current = canonical_root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => current.push(name),
            _ => anyhow::bail!(
                "{} is outside the checkout {}; nothing was written",
                source.display(),
                root.display()
            ),
        }
        let metadata = std::fs::symlink_metadata(&current).with_context(|| {
            format!("cannot inspect {}; nothing was written", current.display())
        })?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "the requested Go source contains linked source path {}; deletion is refused",
            current.display()
        );
    }
    Ok(())
}

fn regular_unlinked_inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "{} is not an unlinked regular source file; nothing was written",
        path.display()
    );
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        canonical.starts_with(root),
        "{} is outside the checkout {}; nothing was written",
        path.display(),
        root.display()
    );
    Ok(canonical)
}

fn go_module_root(root: &Path, source: &Path) -> Result<PathBuf> {
    let mut directory = source.parent();
    while let Some(candidate) = directory {
        anyhow::ensure!(
            candidate.starts_with(root),
            "the source is outside the checkout"
        );
        if candidate.join("go.mod").is_file() {
            return Ok(candidate.to_path_buf());
        }
        if candidate == root {
            break;
        }
        directory = candidate.parent();
    }
    anyhow::bail!(
        "no Go module contains {}; nothing was written",
        source.display()
    )
}

fn refuse_linked_sources(module: &Path) -> Result<()> {
    fn visit(directory: &Path) -> Result<Option<PathBuf>> {
        for entry in std::fs::read_dir(directory)
            .with_context(|| format!("cannot inspect Go module {}", directory.display()))?
        {
            let entry = entry.context("cannot inspect a Go module entry")?;
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("cannot inspect {}", path.display()))?;
            if metadata.file_type().is_symlink() {
                let hides_go = path.extension().is_some_and(|extension| extension == "go")
                    || std::fs::metadata(&path).is_ok_and(|target| target.is_dir());
                if hides_go {
                    return Ok(Some(path));
                }
            } else if metadata.is_dir()
                && let Some(link) = visit(&path)?
            {
                return Ok(Some(link));
            }
        }
        Ok(None)
    }
    if let Some(link) = visit(module)? {
        anyhow::bail!(
            "the Go module contains linked source path {}; deletion is refused",
            link.display()
        );
    }
    Ok(())
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
