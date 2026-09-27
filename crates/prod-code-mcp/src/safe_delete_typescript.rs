//! Conservative safe deletion for private TypeScript module functions.
//!
//! Native TypeScript supplies the declaration and complete references. The exact proposal is
//! then compiled in an isolated gateway shadow and applied only while every observed byte is
//! still current.

use anyhow::{Context, Result, anyhow};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedFunction {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
struct FunctionRange {
    name: String,
    start: usize,
    end: usize,
    name_start: usize,
    name_end: usize,
}

struct Project {
    root: PathBuf,
    sources: BTreeSet<PathBuf>,
}

struct CompileVerdict {
    passed: bool,
    output: String,
}

pub async fn delete_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<DeletedFunction> {
    anyhow::ensure!(
        file.extension().is_some_and(|extension| extension == "ts")
            && !file
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".d.ts")),
        "{} is not a supported TypeScript source file; nothing was written",
        file.display()
    );
    let checkout = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve checkout {}", root.display()))?;
    refuse_linked_path(root, &checkout, file)?;
    let file = regular_unlinked_inside(&checkout, file)?;
    let project = inspect_project(&checkout, &file)?;
    let original = std::fs::read_to_string(&file).with_context(|| {
        format!(
            "cannot read {}; nothing was written",
            display(&checkout, &file)
        )
    })?;
    inspect_source(&original)?;
    anyhow::ensure!(
        module_marker(&original)?,
        "{} is not a contained TypeScript ES module; nothing was written",
        display(&checkout, &file)
    );
    anyhow::ensure!(
        line > 0 && col > 0,
        "the requested position is not one-based"
    );
    let requested = offset_at(&original, line - 1, col - 1).with_context(|| {
        format!(
            "{}:{line}:{col} is not a valid UTF-16 source position; nothing was written",
            display(&checkout, &file)
        )
    })?;

    let observed = project_snapshot(&project.root)?;
    let uri = url::Url::from_file_path(&file)
        .map_err(|_| anyhow!("invalid TypeScript source path {}", file.display()))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        &checkout,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
    .context(
        "the TypeScript language server could not describe declarations; nothing was written",
    )?;
    refuse_incomplete_evidence(&checkout, "declaration")?;
    unchanged(&project.root, &observed, "declarations")?;
    let function = function_at(&original, requested, &symbols)
        .context("TypeScript safe delete refused; nothing was written")?;

    reference_evidence(remote, &checkout, &project, &file, &original, &function)
        .await
        .context("TypeScript safe delete refused; nothing was written")?;
    refuse_incomplete_evidence(&checkout, "reference")?;
    unchanged(&project.root, &observed, "references")?;

    let mut proposed = original.clone();
    proposed.replace_range(function.start..function.end, "");
    let verdict = compile_typescript_shadow(remote, &checkout, &project.root, &file, &proposed)
        .await
        .context("TypeScript safe delete compiler evidence is unavailable; nothing was written")?;
    anyhow::ensure!(
        verdict.passed,
        "deleting {} does not compile under the active TypeScript configuration; nothing was written:\n{}",
        function.name,
        verdict.output.trim()
    );
    unchanged(&project.root, &observed, "the compiler check")?;

    let mut files = BTreeMap::new();
    files.insert(file.clone(), proposed);
    let touched = crate::refactor::apply_workspace_edit(
        &checkout,
        &crate::signature::whole_file_edit(&files),
    )
    .context("the verified TypeScript deletion could not be applied")?;
    anyhow::ensure!(
        touched.len() == 1,
        "the verified TypeScript deletion updated {} paths instead of one",
        touched.len()
    );
    Ok(DeletedFunction {
        name: function.name,
        path: file,
    })
}

fn function_at(text: &str, requested: usize, answer: &serde_json::Value) -> Result<FunctionRange> {
    let symbols = answer.as_array().with_context(|| {
        format!("textDocument/documentSymbol returned no usable list: {answer}")
    })?;
    let mut selected = Vec::new();
    collect_selected(text, requested, symbols, &mut selected)?;
    anyhow::ensure!(
        selected.len() == 1,
        "the position is on {} declaration names instead of exactly one",
        selected.len()
    );
    let (name, kind, range, selection) = selected.pop().expect("one selected symbol");
    anyhow::ensure!(
        kind == 12,
        "the position names a non-function declaration, not an ordinary function"
    );
    parse_function(text, &name, range, selection)
}

fn collect_selected(
    text: &str,
    requested: usize,
    symbols: &[serde_json::Value],
    selected: &mut Vec<(String, u64, Span, Span)>,
) -> Result<()> {
    for symbol in symbols {
        let name = symbol
            .get("name")
            .and_then(serde_json::Value::as_str)
            .context("a document symbol has no name")?;
        let kind = symbol
            .get("kind")
            .and_then(serde_json::Value::as_u64)
            .context("a document symbol has no kind")?;
        anyhow::ensure!(
            !name.is_empty() && (1..=26).contains(&kind),
            "a document symbol is malformed: {symbol}"
        );
        anyhow::ensure!(
            symbol.get("location").is_none(),
            "SymbolInformation without an exact selectionRange is not sufficient TypeScript declaration evidence"
        );
        let range = lsp_span(
            text,
            symbol
                .get("range")
                .context("a document symbol has no declaration range")?,
        )
        .context("a document symbol has a malformed declaration range")?;
        let selection = lsp_span(
            text,
            symbol
                .get("selectionRange")
                .context("a document symbol has no selection range")?,
        )
        .context("a document symbol has a malformed selection range")?;
        anyhow::ensure!(
            range.start <= selection.start
                && selection.start < selection.end
                && selection.end <= range.end,
            "a document symbol's selection range is outside its declaration range"
        );
        if selection.start <= requested && requested < selection.end {
            selected.push((name.to_string(), kind, range, selection));
        }
        match symbol.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => {
                collect_selected(text, requested, children, selected)?
            }
            Some(_) => anyhow::bail!("a document symbol has malformed children"),
        }
    }
    Ok(())
}

fn parse_function(
    text: &str,
    symbol_name: &str,
    analyzer_range: Span,
    selection: Span,
) -> Result<FunctionRange> {
    let source_name = text
        .get(selection.start..selection.end)
        .context("the declaration selection range is stale")?;
    anyhow::ensure!(
        source_name == symbol_name,
        "the analyzer's symbol name {symbol_name:?} does not match {source_name:?}"
    );
    anyhow::ensure!(
        ascii_identifier(source_name),
        "{source_name:?} is not a supported ASCII TypeScript function name"
    );
    anyhow::ensure!(
        analyzer_range.start < analyzer_range.end && analyzer_range.end <= text.len(),
        "the declaration range is empty or outside the source"
    );
    let start = analyzer_range.start;
    anyhow::ensure!(
        keyword_at(text, start, "function"),
        "the declaration range does not begin at an ordinary function keyword"
    );
    ensure_top_level(text, start)?;
    refuse_export_or_modifier(text, start)?;

    let mut cursor = skip_trivia(text, start + "function".len())?;
    anyhow::ensure!(
        text.as_bytes().get(cursor) != Some(&b'*'),
        "generator functions are not supported"
    );
    let name_start = cursor;
    let name_end = ascii_identifier_end(text, name_start)
        .context("the function declaration has no ordinary ASCII name")?;
    anyhow::ensure!(
        name_start == selection.start
            && name_end == selection.end
            && &text[name_start..name_end] == source_name,
        "the analyzer's selection range does not match the current function declaration"
    );
    cursor = skip_trivia(text, name_end)?;
    anyhow::ensure!(
        text.as_bytes().get(cursor) != Some(&b'<'),
        "generic TypeScript functions are not supported"
    );
    anyhow::ensure!(
        text.as_bytes().get(cursor) == Some(&b'('),
        "the function has no parameter list"
    );
    cursor = matching(text, cursor)? + 1;
    loop {
        cursor = skip_trivia(text, cursor)?;
        let byte = *text
            .as_bytes()
            .get(cursor)
            .context("the function declaration has no complete body")?;
        match byte {
            b'{' => break,
            b';' | b'=' => {
                anyhow::bail!("ambient declarations and overload signatures are not supported")
            }
            b'<' => anyhow::bail!("generic TypeScript functions are not supported"),
            b'/' => anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            ),
            b'(' | b'[' => cursor = matching(text, cursor)? + 1,
            b':' | b'?' | b'|' | b'&' | b'.' | b',' => cursor += 1,
            b'\'' | b'"' | b'\x60' => {
                cursor = opaque_end(text, cursor)?
                    .context("unterminated literal in the function signature")?
            }
            value if value.is_ascii_alphanumeric() || value == b'_' || value == b'$' => {
                cursor = ascii_identifier_end(text, cursor)
                    .context("the function signature contains a non-ASCII identifier")?;
            }
            _ => {
                anyhow::bail!("the function return type is outside the supported simple subset")
            }
        }
    }
    let body_end = matching(text, cursor)? + 1;
    anyhow::ensure!(
        analyzer_range.end == body_end,
        "the analyzer's declaration range does not exactly match the current function body"
    );
    Ok(FunctionRange {
        name: source_name.to_string(),
        start,
        end: body_end,
        name_start,
        name_end,
    })
}

fn refuse_export_or_modifier(text: &str, function_start: usize) -> Result<()> {
    let mut cursor = 0usize;
    while cursor < function_start {
        if text.as_bytes()[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        if keyword_at(text, cursor, "export")
            || keyword_at(text, cursor, "declare")
            || keyword_at(text, cursor, "async")
        {
            let keyword_end = ascii_identifier_end(text, cursor).expect("known keyword");
            let mut next = skip_trivia(text, keyword_end)?;
            if keyword_at(text, cursor, "export") && keyword_at(text, next, "default") {
                next = skip_trivia(text, next + "default".len())?;
            }
            if next == function_start {
                anyhow::bail!("exported, ambient, and async functions are not supported");
            }
        }
        cursor = advance(text, cursor);
    }
    Ok(())
}

async fn reference_evidence(
    remote: SocketAddr,
    checkout: &Path,
    project: &Project,
    file: &Path,
    text: &str,
    function: &FunctionRange,
) -> Result<()> {
    let (line, character) = line_col_utf16(text, function.name_start)?;
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow!("invalid TypeScript source path {}", file.display()))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        checkout,
        file,
        "textDocument/references",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true }
        }),
    )
    .await
    .context("the TypeScript language server could not list every reference")?;
    let locations = answer
        .as_array()
        .filter(|locations| !locations.is_empty())
        .with_context(|| {
            format!(
                "the TypeScript language server listed no location, not even the declaration of {}: {answer}",
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
            "a reference is not a plain local file URI"
        );
        let reported = uri
            .to_file_path()
            .map_err(|_| anyhow!("a reference is not a local file URI"))?;
        let path = regular_unlinked_inside(&project.root, &reported)?;
        anyhow::ensure!(
            project.sources.contains(&path),
            "{} is outside the complete configured TypeScript source set",
            display(checkout, &path)
        );
        let source = if path == file {
            text.to_string()
        } else {
            std::fs::read_to_string(&path).with_context(|| {
                format!("cannot read referenced source {}", display(checkout, &path))
            })?
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
            display(checkout, &path)
        );
        anyhow::ensure!(
            seen.insert((path.clone(), span.start, span.end)),
            "{} contains duplicate reference evidence",
            display(checkout, &path)
        );
        if path == file && span.start == function.name_start && span.end == function.name_end {
            declarations += 1;
        } else {
            let (line, col) = line_col_utf16(&source, span.start)?;
            uses.push(format!(
                "{}:{}:{}",
                display(checkout, &path),
                line + 1,
                col + 1
            ));
        }
    }
    anyhow::ensure!(
        declarations == 1,
        "the TypeScript language server listed the declaration {} times instead of exactly once",
        declarations
    );
    anyhow::ensure!(
        uses.is_empty(),
        "{} is still referenced at {}; no function was deleted",
        function.name,
        uses.join(", ")
    );
    Ok(())
}

async fn compile_typescript_shadow(
    remote: SocketAddr,
    checkout: &Path,
    project: &Path,
    source: &Path,
    proposed: &str,
) -> Result<CompileVerdict> {
    let observed = project_snapshot(project)?;
    let relative_project = project
        .strip_prefix(checkout)
        .expect("project is inside checkout");
    let subdir = if relative_project.as_os_str().is_empty() {
        None
    } else {
        Some(
            relative_project
                .to_str()
                .context("the TypeScript project path is not UTF-8")?,
        )
    };
    let outcome = crate::shadow::run_shadow(
        remote,
        checkout,
        subdir,
        &[crate::shadow::HypothesisSpec {
            name: "typescript-compiler-verification".to_string(),
            edits: vec![crate::shadow::HypothesisEdit {
                relative_path: crate::shadow::relative_edit_path(checkout, source)?,
                text: Some(proposed.to_string()),
            }],
        }],
        [
            "tsc",
            "--noEmit",
            "--pretty",
            "false",
            "--incremental",
            "false",
            "--project",
            "tsconfig.json",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        Vec::new(),
        120,
        1,
        16 * 1024,
    )
    .await
    .context("the remote gateway could not run TypeScript compiler verification")?;
    anyhow::ensure!(
        matches!(outcome.mode.as_str(), "overlay" | "in-place"),
        "the remote gateway returned an unrecognized shadow mode {:?}",
        outcome.mode
    );
    anyhow::ensure!(
        outcome.results.len() == 1,
        "the remote gateway returned {} compiler outcomes instead of one",
        outcome.results.len()
    );
    let result = &outcome.results[0];
    anyhow::ensure!(
        result.name == "typescript-compiler-verification",
        "the remote gateway returned compiler evidence for {:?}",
        result.name
    );
    anyhow::ensure!(
        result.error.is_none(),
        "the remote TypeScript compiler could not start: {}",
        result.error.as_deref().unwrap_or_default()
    );
    anyhow::ensure!(
        !result.timed_out,
        "the remote TypeScript compiler timed out"
    );
    let exit_code = result
        .exit_code
        .context("the remote TypeScript compiler returned no exit status")?;
    unchanged(project, &observed, "the remote compiler")?;
    let mut output = result.output.clone();
    if !outcome.server_workspace_root.is_empty() {
        output = output.replace(&outcome.server_workspace_root, ".");
    }
    output = output.replace(checkout.to_string_lossy().as_ref(), ".");
    if exit_code != 0 && output.trim().is_empty() {
        output = format!("the TypeScript compiler exited with {exit_code} and no diagnostic");
    }
    Ok(CompileVerdict {
        passed: exit_code == 0,
        output,
    })
}

fn inspect_project(checkout: &Path, source: &Path) -> Result<Project> {
    let mut directory = source.parent();
    let config = loop {
        let candidate = directory.context("the TypeScript source has no parent directory")?;
        anyhow::ensure!(
            candidate.starts_with(checkout),
            "the TypeScript source is outside the checkout"
        );
        let config = candidate.join("tsconfig.json");
        if config.exists() {
            break config;
        }
        if candidate == checkout {
            anyhow::bail!(
                "no tsconfig.json contains {}; nothing was written",
                display(checkout, source)
            );
        }
        directory = candidate.parent();
    };
    refuse_linked_path(checkout, checkout, &config)?;
    let config = regular_unlinked_inside(checkout, &config)?;
    let project_root = config.parent().expect("config has parent").to_path_buf();
    let raw = std::fs::read_to_string(&config).with_context(|| {
        format!(
            "cannot read {}; nothing was written",
            display(checkout, &config)
        )
    })?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .context("tsconfig.json must be strict JSON in the supported safe-delete subset")?;
    let object = json
        .as_object()
        .context("tsconfig.json must contain an object")?;
    for unsafe_key in ["extends", "references", "files", "exclude"] {
        anyhow::ensure!(
            !object.contains_key(unsafe_key),
            "tsconfig.json field {unsafe_key:?} is outside the supported contained configuration"
        );
    }
    anyhow::ensure!(
        object
            .keys()
            .all(|key| matches!(key.as_str(), "compilerOptions" | "include")),
        "tsconfig.json contains fields outside the supported simple configuration"
    );
    let options = object
        .get("compilerOptions")
        .and_then(serde_json::Value::as_object)
        .context("tsconfig.json needs compilerOptions")?;
    for unsafe_key in [
        "allowJs",
        "checkJs",
        "composite",
        "incremental",
        "declaration",
        "emitDeclarationOnly",
        "outDir",
        "rootDir",
        "rootDirs",
        "paths",
        "baseUrl",
        "typeRoots",
        "plugins",
    ] {
        anyhow::ensure!(
            !options.contains_key(unsafe_key),
            "compiler option {unsafe_key:?} is outside the supported contained configuration"
        );
    }
    let module = options
        .get("module")
        .and_then(serde_json::Value::as_str)
        .context("compilerOptions.module must explicitly select an ES module mode")?;
    anyhow::ensure!(
        matches!(
            module.to_ascii_lowercase().as_str(),
            "es2020" | "es2022" | "esnext" | "node16" | "nodenext"
        ),
        "compilerOptions.module is not a supported ES module mode"
    );
    let includes = object
        .get("include")
        .and_then(serde_json::Value::as_array)
        .filter(|items| !items.is_empty())
        .context("tsconfig.json needs a non-empty include list")?;
    let mut roots = BTreeSet::new();
    for include in includes {
        let include = include
            .as_str()
            .context("every tsconfig include entry must be a string")?;
        let prefix = include
            .strip_suffix("/**/*.ts")
            .or_else(|| include.strip_suffix("/**/*"))
            .unwrap_or(include)
            .trim_end_matches('/');
        anyhow::ensure!(
            !prefix.is_empty()
                && !prefix.contains('*')
                && !prefix.contains('?')
                && !Path::new(prefix).is_absolute()
                && Path::new(prefix).components().all(|component| {
                    matches!(
                        component,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                }),
            "tsconfig include {include:?} is outside the supported directory subset"
        );
        let requested = project_root.join(prefix);
        refuse_linked_path(&project_root, &project_root, &requested)?;
        let canonical = std::fs::canonicalize(&requested)
            .with_context(|| format!("cannot resolve included TypeScript directory {include:?}"))?;
        anyhow::ensure!(
            canonical.starts_with(&project_root) && canonical.is_dir(),
            "included TypeScript path {include:?} is not a directory inside the project"
        );
        roots.insert(canonical);
    }

    let mut sources = BTreeSet::new();
    for root in roots {
        collect_sources(&project_root, &root, &mut sources)?;
    }
    anyhow::ensure!(
        sources.contains(source),
        "{} is not in the tsconfig include set",
        display(checkout, source)
    );
    for path in &sources {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!("cannot read configured source {}", display(checkout, path))
        })?;
        inspect_source(&text).with_context(|| {
            format!(
                "configured source {} is unsupported",
                display(checkout, path)
            )
        })?;
    }
    Ok(Project {
        root: project_root,
        sources,
    })
}

fn collect_sources(
    project: &Path,
    directory: &Path,
    sources: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("cannot inspect TypeScript project {}", directory.display()))?
    {
        let entry = entry.context("cannot inspect a TypeScript project entry")?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "the TypeScript project contains linked path {}; deletion is refused",
            display(project, &path)
        );
        if metadata.is_dir() {
            collect_sources(project, &path, sources)?;
            continue;
        }
        let extension = path.extension().and_then(|value| value.to_str());
        if matches!(extension, Some("js" | "jsx" | "mjs" | "cjs" | "tsx")) {
            anyhow::bail!(
                "{} is JavaScript or TSX; only contained .ts projects are supported",
                display(project, &path)
            );
        }
        if extension == Some("ts") {
            anyhow::ensure!(
                !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".d.ts")),
                "ambient declaration source {} is not supported",
                display(project, &path)
            );
            sources.insert(std::fs::canonicalize(&path)?);
        }
    }
    Ok(())
}

fn inspect_source(text: &str) -> Result<()> {
    let header = text
        .get(..text.len().min(4096))
        .unwrap_or(text)
        .to_ascii_lowercase();
    anyhow::ensure!(
        !header.contains("@generated")
            && !header.contains("generated file")
            && !header.contains("do not edit")
            && !text.contains("sourceMappingURL="),
        "generated TypeScript source is not supported"
    );
    let mut cursor = 0usize;
    while cursor < text.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        if text.as_bytes()[cursor] == b'/' {
            anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            );
        }
        if let Some(end) = ascii_identifier_end(text, cursor) {
            let word = &text[cursor..end];
            let next = skip_trivia(text, end)?;
            if word == "eval"
                || (matches!(word, "Function" | "require" | "import")
                    && text.as_bytes().get(next) == Some(&b'('))
            {
                anyhow::bail!("dynamic evaluation or name resolution is not supported");
            }
            cursor = end;
        } else {
            anyhow::ensure!(
                text.as_bytes()[cursor] != b'\\',
                "escaped identifiers are outside the safe-delete subset"
            );
            cursor = advance(text, cursor);
        }
    }
    Ok(())
}

fn module_marker(text: &str) -> Result<bool> {
    let mut cursor = 0usize;
    let mut stack = Vec::new();
    while cursor < text.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        let byte = text.as_bytes()[cursor];
        match byte {
            b'(' | b'[' | b'{' => stack.push(byte),
            b')' | b']' | b'}' => {
                let open = stack
                    .pop()
                    .context("unmatched delimiter in TypeScript source")?;
                anyhow::ensure!(
                    pair(open, byte),
                    "mismatched delimiter in TypeScript source"
                );
            }
            _ if stack.is_empty()
                && (keyword_at(text, cursor, "import") || keyword_at(text, cursor, "export")) =>
            {
                return Ok(true);
            }
            _ => {}
        }
        cursor = advance(text, cursor);
    }
    anyhow::ensure!(stack.is_empty(), "unclosed delimiter in TypeScript source");
    Ok(false)
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
        let byte = text.as_bytes()[cursor];
        if byte == b'/' {
            anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            );
        }
        match byte {
            b'(' | b'[' | b'{' => stack.push(byte),
            b')' | b']' | b'}' => {
                let open = stack
                    .pop()
                    .context("unmatched closing delimiter before the function")?;
                anyhow::ensure!(pair(open, byte), "mismatched delimiter before the function");
            }
            _ => {}
        }
        cursor = advance(text, cursor);
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
        let byte = text.as_bytes()[cursor];
        if byte == b'/' {
            anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            );
        }
        match byte {
            b'(' | b'[' | b'{' => stack.push(byte),
            b')' | b']' | b'}' => {
                let opened = stack
                    .pop()
                    .context("an unmatched delimiter closes the declaration")?;
                anyhow::ensure!(
                    pair(opened, byte),
                    "a delimiter is mismatched in the declaration"
                );
                if stack.is_empty() {
                    return Ok(cursor);
                }
            }
            _ => {}
        }
        cursor = advance(text, cursor);
    }
    anyhow::bail!("an opening delimiter in the declaration is not closed")
}

fn opaque_end(text: &str, start: usize) -> Result<Option<usize>> {
    let bytes = text.as_bytes();
    let Some(&first) = bytes.get(start) else {
        return Ok(None);
    };
    let end = match first {
        quote @ (b'\'' | b'"') => {
            let mut cursor = start + 1;
            loop {
                let byte = *bytes
                    .get(cursor)
                    .context("a quoted literal is not terminated")?;
                match byte {
                    b'\\' => {
                        let escaped = cursor
                            .checked_add(1)
                            .filter(|escaped| *escaped < text.len())
                            .context("a quoted escape is truncated")?;
                        cursor = advance(text, escaped);
                    }
                    b'\n' | b'\r' => {
                        anyhow::bail!("a quoted literal crosses a line without closing")
                    }
                    value if value == quote => break cursor + 1,
                    _ => cursor = advance(text, cursor),
                }
            }
        }
        b'\x60' => {
            let mut cursor = start + 1;
            loop {
                let byte = *bytes
                    .get(cursor)
                    .context("a template literal is not terminated")?;
                match byte {
                    b'\\' => {
                        let escaped = cursor
                            .checked_add(1)
                            .filter(|escaped| *escaped < text.len())
                            .context("a template escape is truncated")?;
                        cursor = advance(text, escaped);
                    }
                    b'$' if bytes.get(cursor + 1) == Some(&b'{') => {
                        anyhow::bail!("template expressions are outside the safe-delete subset")
                    }
                    b'\x60' => break cursor + 1,
                    _ => cursor = advance(text, cursor),
                }
            }
        }
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
            Some(end) if text.as_bytes().get(cursor) == Some(&b'/') => cursor = end,
            _ => return Ok(cursor),
        }
    }
}

fn keyword_at(text: &str, start: usize, word: &str) -> bool {
    text.get(start..start + word.len()) == Some(word)
        && token_boundary(text.as_bytes(), start, word.len())
}

fn ascii_identifier_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)?;
    if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'$')) {
        return None;
    }
    let mut end = start + 1;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'$'))
    {
        end += 1;
    }
    Some(end)
}

fn ascii_identifier(name: &str) -> bool {
    ascii_identifier_end(name, 0) == Some(name.len())
        && !matches!(
            name,
            "await"
                | "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "enum"
                | "export"
                | "extends"
                | "false"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "let"
                | "new"
                | "null"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
                | "yield"
        )
}

fn token_boundary(bytes: &[u8], start: usize, len: usize) -> bool {
    let identifier =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 0x80;
    !start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| identifier(*byte))
        && !bytes.get(start + len).is_some_and(|byte| identifier(*byte))
}

fn pair(open: u8, close: u8) -> bool {
    matches!((open, close), (b'(', b')') | (b'[', b']') | (b'{', b'}'))
}

fn advance(text: &str, cursor: usize) -> usize {
    cursor
        + text[cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1)
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
    Ok((
        u32::try_from(before.matches('\n').count())?,
        u32::try_from(
            before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .encode_utf16()
                .count(),
        )?,
    ))
}

fn project_snapshot(project: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        for entry in std::fs::read_dir(directory)
            .with_context(|| format!("cannot snapshot {}", directory.display()))?
        {
            let entry = entry.context("cannot inspect a TypeScript project entry")?;
            let path = entry.path();
            let name = path.file_name().and_then(|name| name.to_str());
            if matches!(name, Some(".git" | "node_modules" | "target")) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("cannot inspect {}", path.display()))?;
            anyhow::ensure!(
                !metadata.file_type().is_symlink(),
                "the TypeScript project contains linked path {}; deletion is refused",
                display(root, &path)
            );
            if metadata.is_dir() {
                visit(root, &path, files)?;
            } else if metadata.is_file() {
                files.insert(
                    path.strip_prefix(root)
                        .expect("entry below root")
                        .to_path_buf(),
                    std::fs::read(&path)
                        .with_context(|| format!("cannot snapshot {}", path.display()))?,
                );
            } else {
                anyhow::bail!("the TypeScript project contains a special file");
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(project, project, &mut files)?;
    Ok(files)
}

fn unchanged(project: &Path, observed: &BTreeMap<PathBuf, Vec<u8>>, stage: &str) -> Result<()> {
    anyhow::ensure!(
        project_snapshot(project)? == *observed,
        "the TypeScript project changed while {stage} was inspected; nothing was written"
    );
    Ok(())
}

fn refuse_incomplete_evidence(root: &Path, stage: &str) -> Result<()> {
    let notes = crate::session::take_indexing_notes(root);
    anyhow::ensure!(
        notes.is_empty(),
        "the TypeScript language server reported incomplete {stage} evidence: {}",
        notes.join("; ")
    );
    Ok(())
}

fn refuse_linked_path(root: &Path, canonical_root: &Path, source: &Path) -> Result<()> {
    let relative = source
        .strip_prefix(root)
        .or_else(|_| source.strip_prefix(canonical_root))
        .with_context(|| {
            format!(
                "{} is outside the checkout; nothing was written",
                source.display()
            )
        })?;
    let mut current = canonical_root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => current.push(name),
            _ => anyhow::bail!(
                "{} is outside the checkout; nothing was written",
                source.display()
            ),
        }
        let metadata = std::fs::symlink_metadata(&current).with_context(|| {
            format!("cannot inspect {}; nothing was written", current.display())
        })?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "the requested TypeScript path contains linked path {}; deletion is refused",
            current.display()
        );
    }
    Ok(())
}

fn regular_unlinked_inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("cannot inspect {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "{} is not an unlinked regular source file; nothing was written",
        path.display()
    );
    let canonical = std::fs::canonicalize(&path)
        .with_context(|| format!("cannot resolve {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        canonical.starts_with(root),
        "{} is outside the TypeScript project; nothing was written",
        path.display()
    );
    Ok(canonical)
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
