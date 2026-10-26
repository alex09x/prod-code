/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::{cell::RefCell, collections::BTreeMap, ops::Range};

use anyhow::{Context, Result, ensure};
use oxc_allocator::Allocator;
use oxc_ast::{AstKind, ast::ClassType};
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::{GetSpan, SourceType};

use crate::{extract_delegate::common::is_ident, parameter_object::Language};

const CTOR_WRAP: (&str, &str) = ("class __O extends __B {\nconstructor() {\n", "\n}\n}\n");
const EXPR_WRAP: (&str, &str) = (
    "class __O extends __B {\nconstructor() {\nconst __v = (\n",
    "\n);\n}\n}\n",
);

struct ParsedClass(String, usize, bool, usize, usize);

struct ParsedSource {
    non_code: Vec<Range<usize>>,
    regex_ends: BTreeMap<usize, usize>,
    classes: Vec<ParsedClass>,
    has_owner_ref: bool,
}

thread_local! {
    static PARSED_CACHE: RefCell<Option<(String, Option<ParsedSource>)>> = const { RefCell::new(None) };
}

fn source_types() -> [SourceType; 6] {
    let (mjs, cjs) = (SourceType::mjs(), SourceType::cjs());
    [
        SourceType::ts(),
        SourceType::tsx(),
        mjs,
        mjs.with_jsx(true),
        cjs,
        cjs.with_jsx(true),
    ]
}

fn parse_program(text: &str) -> Option<ParsedSource> {
    let try_parse = |s: &str, off, collect| {
        source_types()
            .into_iter()
            .find_map(|st| parse_with_type(s, off, text.len(), st, collect))
    };
    try_parse(text, 0, true).or_else(|| {
        [("const __prod_code_expr = (\n", "\n);\n"), CTOR_WRAP]
            .into_iter()
            .find_map(|(p, s)| try_parse(&format!("{p}{text}{s}"), p.len(), false))
    })
}

fn strip_kw<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    s.strip_suffix(kw)
        .filter(|r| !r.chars().next_back().is_some_and(is_ident))
}

fn class_header_bounds(
    src: &str,
    (span_start, after_dec, id_start, exp): (usize, usize, usize, Option<usize>),
    non_code: &[Range<usize>],
) -> (bool, usize) {
    if let Some(exp) = exp {
        return (true, exp.min(span_start));
    }
    let class_kw = src.get(after_dec..id_start).map_or(span_start, |s| {
        s.match_indices("class")
            .map(|(rel, _)| after_dec + rel)
            .find(|&p| {
                !non_code.iter().any(|r| r.contains(&p))
                    && !src[..p].chars().next_back().is_some_and(is_ident)
                    && !src[p + 5..].chars().next().is_some_and(is_ident)
            })
            .unwrap_or(span_start)
    });
    let mut h = src[..class_kw].trim_end();
    while let Some(r) = strip_kw(h, "abstract").or_else(|| strip_kw(h, "declare")) {
        h = r.trim_end();
    }
    let h = strip_kw(h, "default").map_or(h, str::trim_end);
    strip_kw(h, "export").map_or((false, span_start.min(class_kw)), |p| {
        (true, p.len().min(span_start))
    })
}

fn parse_with_type(
    source: &str,
    off: usize,
    len: usize,
    source_type: SourceType,
    collect_classes: bool,
) -> Option<ParsedSource> {
    let allocator = Allocator::default();
    let opts = ParseOptions {
        preserve_parens: true,
        ..Default::default()
    };
    let parsed = Parser::new(&allocator, source, source_type)
        .with_options(opts)
        .parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return None;
    }
    let built = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program);
    let end = off + len;
    let mut non_code = Vec::new();
    let mut push = |s: usize, e: usize| {
        let (s, e) = (s.max(off) - off, e.min(end).saturating_sub(off));
        if s < e {
            non_code.push(s..e);
        }
    };
    let (mut regex_ends, mut raw_classes, mut has_owner_ref) = (BTreeMap::new(), Vec::new(), false);
    if let Some(hb) = &parsed.program.hashbang {
        push(hb.span.start as usize, hb.span.end as usize);
    }
    for c in &parsed.program.comments {
        let (mut s, mut e) = (c.span.start as usize, c.span.end as usize);
        s -= 2 * usize::from(source[..s].ends_with("//") || source[..s].ends_with("/*"));
        e += 2 * usize::from(source[e..].starts_with("*/"));
        push(s, e);
    }
    for d in &parsed.program.directives {
        push(
            d.expression.span.start as usize + 1,
            (d.expression.span.end as usize).saturating_sub(1),
        );
    }
    for node in built.semantic.nodes().iter() {
        let span = node.kind().span();
        if (span.start as usize) >= off
            && (span.end as usize) <= end
            && matches!(node.kind(), AstKind::ThisExpression(_) | AstKind::Super(_))
        {
            has_owner_ref = true;
        }
        match node.kind() {
            AstKind::StringLiteral(lit) => push(
                lit.span.start as usize + 1,
                (lit.span.end as usize).saturating_sub(1),
            ),
            AstKind::JSXText(txt) => push(txt.span.start as usize, txt.span.end as usize),
            AstKind::RegExpLiteral(reg) => {
                let (s, e) = (reg.span.start as usize, reg.span.end as usize);
                if let Some(rel) = source[s..e].rfind('/').filter(|&r| r > 0) {
                    push(s + 1, s + rel + 1);
                    if s >= off && s + rel < end {
                        regex_ends.insert(s - off, s + rel + 1 - off);
                    }
                }
            }
            AstKind::TemplateLiteral(tpl) => {
                for q in &tpl.quasis {
                    let (qs, qe) = (q.span.start as usize, q.span.end as usize);
                    let s = qs - usize::from(qs != tpl.span.start as usize + 1);
                    push(s, (qe + 2 * usize::from(!q.tail)).min(source.len()));
                }
            }
            AstKind::Class(c) if collect_classes && c.r#type == ClassType::ClassDeclaration => {
                let (open, close) = (
                    c.body.span.start as usize,
                    (c.body.span.end as usize).saturating_sub(1),
                );
                if let Some(id) = c.id.as_ref().filter(|_| {
                    open < close
                        && source.as_bytes().get(open) == Some(&b'{')
                        && source.as_bytes().get(close) == Some(&b'}')
                }) {
                    let s = c.span.start as usize;
                    let dec = c.decorators.last().map_or(s, |d| d.span.end as usize);
                    let pk = built.semantic.nodes().parent_kind(node.id());
                    let exp = matches!(
                        pk,
                        AstKind::ExportNamedDeclaration(_) | AstKind::ExportDefaultDeclaration(_)
                    )
                    .then(|| pk.span().start as usize);
                    raw_classes.push((
                        id.name.as_str().to_string(),
                        (s, dec, id.span.start as usize, exp),
                        open,
                        close,
                    ));
                }
            }
            _ => {}
        }
    }
    let classes = raw_classes
        .into_iter()
        .map(|(name, bounds, open, close)| {
            let (is_export, ins) = class_header_bounds(source, bounds, &non_code);
            ParsedClass(name, ins, is_export, open, close)
        })
        .collect();
    Some(ParsedSource {
        non_code,
        regex_ends,
        classes,
        has_owner_ref,
    })
}

fn with_parsed<R>(text: &str, f: impl FnOnce(Option<&ParsedSource>) -> R) -> R {
    PARSED_CACHE.with(|cache| {
        if !cache.borrow().as_ref().is_some_and(|(t, _)| t == text) {
            *cache.borrow_mut() = Some((text.to_string(), parse_program(text)));
        }
        let borrow = cache.borrow();
        f(borrow.as_ref().and_then(|(_, p)| p.as_ref()))
    })
}

/// Select the requested class by symbol and line using actual syntax spans before
/// enforcing the selected class's shared-scanner boundary invariant.
pub(super) fn selected_class(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    lang: Language,
) -> Result<(usize, usize, String, bool, usize)> {
    with_parsed(text, |parsed| {
        let Some(parsed) = parsed else {
            return Ok(None);
        };
        for &ParsedClass(ref name, ins, is_export, open, close) in &parsed.classes {
            if symbol_opt.is_some_and(|sym| sym != name) {
                continue;
            }
            if let Some(line) = line_opt.filter(|l| *l > 0) {
                let (sl, _) = crate::signature::position_at(text, ins)?;
                let (el, _) = crate::signature::position_at(text, close)?;
                if line < sl || line > el {
                    continue;
                }
            }
            ensure!(
                crate::parameter_object::syntax::close_in(text, open, lang) == Some(close)
                    && closing_brace_in(text, open, parsed) == Some(close),
                "class has unproved template/regex lexical boundaries"
            );
            return Ok(Some((open, close, name.clone(), is_export, ins)));
        }
        Ok(None)
    })?
    .with_context(|| match symbol_opt {
        Some(sym) => format!("class `{sym}` not found"),
        None => "no class found at the specified location".to_string(),
    })
}

/// Check whether a bounded initializer expression or constructor statement prefix
/// contains an executable `this` or `super` evaluation. Fails closed on unparseable text.
pub(super) fn owner_dependent(text: &str, expression: bool) -> bool {
    let t = text.trim();
    let ((prefix, suffix), inner) = if expression {
        (EXPR_WRAP, t.strip_suffix(';').unwrap_or(t).trim())
    } else {
        (CTOR_WRAP, text)
    };
    let wrapped = format!("{prefix}{inner}{suffix}");
    source_types()
        .into_iter()
        .find_map(|st| parse_with_type(&wrapped, prefix.len(), inner.len(), st, false))
        .map_or(true, |p| p.has_owner_ref)
}

/// Locate executable JS/TS positions using parsed AST literal/comment/template spans,
/// falling back to bounded lexical scanning only for unparseable header slices.
pub(super) fn reference_is_code(text: &str, at: usize) -> bool {
    at < text.len()
        && with_parsed(text, |parsed| match parsed {
            Some(parsed) => !parsed.non_code.iter().any(|r| r.contains(&at)),
            None => code(text.as_bytes(), &mut 0, at, false),
        })
}

/// Recover a regex literal's expression end for ASI decisions.
pub(super) fn regex_literal_end(text: &str, start: usize) -> Option<usize> {
    (text.as_bytes().get(start) == Some(&b'/')).then(|| {
        with_parsed(text, |p| match p {
            Some(p) => p.regex_ends.get(&start).copied(),
            None => {
                let (b, end) = (text.as_bytes(), regex_end(text.as_bytes(), start));
                (end > start + 1
                    && b.get(end - 1) == Some(&b'/')
                    && !code(b, &mut 0, end - 1, false))
                .then_some(end)
            }
        })
    })?
}

fn closing_brace_in(text: &str, open: usize, parsed: &ParsedSource) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text.char_indices().skip_while(|(i, _)| *i < open) {
        if matches!(c, '{' | '}') && !parsed.non_code.iter().any(|r| r.contains(&i)) {
            depth = if c == '{' {
                depth + 1
            } else {
                depth.checked_sub(1)?
            };
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn code(bytes: &[u8], cursor: &mut usize, at: usize, substitution: bool) -> bool {
    let (mut braces, mut operand) = (0usize, true);
    while *cursor < bytes.len() && *cursor <= at {
        let (i, c) = (*cursor, bytes[*cursor]);
        if bytes[i..].starts_with(b"//") {
            *cursor += 2 + bytes[i + 2..].iter().take_while(|&&b| b != b'\n').count();
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            *cursor = bytes[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(bytes.len(), |p| i + 4 + p);
            continue;
        }
        if i == at && !(substitution && c == b'}' && braces == 0) {
            return true;
        }
        if c == b'/' && operand {
            *cursor = regex_end(bytes, i);
            operand = false;
            continue;
        }
        *cursor += 1;
        match c {
            b'\'' | b'"' | b'`' => {
                while *cursor < bytes.len() && (c != b'`' || *cursor <= at) {
                    match bytes[*cursor] {
                        b'\\' => *cursor += 2,
                        b'$' if c == b'`' && bytes.get(*cursor + 1) == Some(&b'{') => {
                            *cursor += 2;
                            if code(bytes, cursor, at, true) {
                                return true;
                            }
                        }
                        end if end == c => {
                            let found = *cursor == at;
                            *cursor += 1;
                            if found {
                                return true;
                            }
                            break;
                        }
                        _ => *cursor += 1,
                    }
                }
                operand = false;
            }
            b'{' => (braces, operand) = (braces + 1, true),
            b'}' if substitution && braces == 0 => return false,
            b'}' => (braces, operand) = (braces.saturating_sub(1), false),
            b')' | b']' | b'0'..=b'9' | b'.' | b'_' | b'$' => operand = false,
            c if c.is_ascii_alphabetic() => operand = false,
            c if !c.is_ascii_whitespace() => operand = true,
            _ => {}
        }
    }
    false
}

fn regex_end(bytes: &[u8], start: usize) -> usize {
    let (mut cursor, mut class) = (start + 1, false);
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'[' => (class, cursor) = (true, cursor + 1),
            b']' => (class, cursor) = (false, cursor + 1),
            b'/' if !class => return cursor + 1,
            b'\n' | b'\r' => return cursor,
            _ => cursor += 1,
        }
    }
    cursor
}
