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
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::parameter_object::Language;

use super::polyglot::{PolyglotShape, format_polyglot_fixture};
use super::resolve::shape_of_polyglot;
use super::types::{Fixture, FixtureOptions, Shape, snake_case};
use super::values::build_literal;

/// Generates a fixture for `symbol`, and type-checks it when asked.
pub async fn generate(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    depth: u32,
    verify: bool,
    hint: Option<&Path>,
) -> Result<Fixture> {
    generate_with_options(
        remote,
        root,
        symbol,
        FixtureOptions {
            depth,
            verify,
            hint: hint.map(PathBuf::from),
            randomized: false,
            mock: false,
            language: None,
        },
    )
    .await
}

/// Generates a fixture or mock with explicit options for polyglot languages, randomized data, and mock mode.
pub async fn generate_with_options(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    options: FixtureOptions,
) -> Result<Fixture> {
    let (shape, file, lang) = shape_of_polyglot(
        remote,
        root,
        symbol,
        options.hint.as_deref(),
        options.language,
    )
    .await?;

    let is_mock = options.mock
        || matches!(
            shape,
            PolyglotShape::Interface { .. } | PolyglotShape::InterfaceWithFields { .. }
        );
    let mut fallbacks = Vec::new();

    let (value, snippet) = if lang == Language::Rust && !options.randomized && !is_mock {
        let rust_shape = match &shape {
            PolyglotShape::Record(f) => Shape::Record(f.clone()),
            PolyglotShape::Tuple(t) => Shape::Tuple(t.clone()),
            PolyglotShape::Unit => Shape::Unit,
            PolyglotShape::Enum(v) => Shape::Enum(v.clone()),
            PolyglotShape::Interface { .. } => Shape::Unit,
            PolyglotShape::InterfaceWithFields { .. } => Shape::Unit,
        };
        let mut seen = vec![symbol.to_string()];
        let val = match &rust_shape {
            Shape::Unit => symbol.to_string(),
            _ => build_literal(
                remote,
                root,
                symbol,
                options.depth,
                0,
                &mut seen,
                &mut fallbacks,
                options.hint.as_deref(),
            )
            .await
            .with_context(|| format!("cannot build a value for `{symbol}`"))?,
        };
        let snip = format!("let {} = {};", snake_case(symbol), val);
        (val, snip)
    } else {
        format_polyglot_fixture(lang, symbol, &shape, options.randomized, is_mock)
    };

    let mut fixture = Fixture {
        type_name: symbol.to_string(),
        value,
        file: file.clone(),
        fallbacks,
        diagnostics: Vec::new(),
        verified: false,
        language: lang.fence(),
        is_mock,
        snippet,
    };

    if options.verify {
        let path = root.join(&file);
        let original = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let first_generated_line = original.lines().count() as u32 + 1;
        let probe = match lang {
            Language::Rust => {
                if fixture.is_mock {
                    format!(
                        "{original}\n#[cfg(test)]\nmod prod_code_generated_fixture {{\n    #[allow(unused_imports)]\n    use super::*;\n\n{}\n}}\n",
                        fixture.snippet
                    )
                } else {
                    format!(
                        "{original}\n#[cfg(test)]\nmod prod_code_generated_fixture {{\n    #[allow(unused_imports)]\n    use super::*;\n\n    #[test]\n    fn builds() {{\n        let {} = {};\n        let _ = {};\n    }}\n}}\n",
                        snake_case(symbol),
                        fixture.value,
                        snake_case(symbol)
                    )
                }
            }
            Language::Go if fixture.is_mock => {
                let is_interface = matches!(shape, PolyglotShape::Interface { .. });
                go_mock_verification_probe(&original, &fixture.snippet, symbol, is_interface)
            }
            Language::Go => {
                format!(
                    "{original}\n\nfunc _TestProdCodeFixtureProbe() {{\n    {}\n}}\n",
                    fixture.snippet
                )
            }
            Language::TypeScript | Language::JavaScript => {
                format!(
                    "{original}\n\n// prod-code fixture probe\n{}\n",
                    fixture.snippet
                )
            }
            Language::Python => {
                format!(
                    "{original}\n\ndef _prod_code_fixture_probe():\n    {}\n",
                    fixture.snippet
                )
            }
            Language::Cpp | Language::C => {
                format!(
                    "{original}\n\nvoid _prod_code_fixture_probe() {{\n    {}\n}}\n",
                    fixture.snippet
                )
            }
            Language::Swift => {
                format!(
                    "{original}\n\nfunc _prod_code_fixture_probe() {{\n    {}\n}}\n",
                    fixture.snippet
                )
            }
            Language::Java => {
                format!(
                    "{original}\n\n// prod-code fixture probe\nclass _ProdCodeFixtureProbe {{\n    void probe() {{\n        {}\n    }}\n}}\n",
                    fixture.snippet
                )
            }
        };

        let reports =
            crate::diagnostics::validate_texts(remote, root, &[(path.clone(), probe)], &[]).await?;
        fixture.verified = true;
        fixture.diagnostics = reports
            .iter()
            .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
            .filter(|(_, d)| d.severity == "error" && d.line >= first_generated_line)
            .map(|(file, d)| {
                format!(
                    "{}{} ({file}:{}:{})",
                    d.message.lines().next().unwrap_or(""),
                    d.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    d.line,
                    d.col
                )
            })
            .collect();
    }

    Ok(fixture)
}

pub(crate) fn go_mock_verification_probe(
    original: &str,
    snippet: &str,
    symbol: &str,
    is_interface: bool,
) -> String {
    let interface_assertion = if is_interface {
        format!("\nvar _ {symbol} = (*Mock{symbol})(nil)\n")
    } else {
        String::new()
    };
    format!(
        "{original}\n\n{snippet}{interface_assertion}\nfunc _TestProdCodeFixtureProbe() {{ _ = &Mock{symbol}{{}} }}\n"
    )
}
