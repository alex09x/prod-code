/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a JS/TS class's construction evaluates, read from its parsed syntax: every instance
//! field initializer, and which receiver a `this` or `super` belongs to (#1409).

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::{NodeId, Semantic, SemanticBuilder};
use oxc_span::{GetSpan, Span};

/// Whether the `this`/`super` at `node` belongs to a function, or a class's field or static
/// block, that starts inside the bounded text at `off`. Such a receiver is the nested code's,
/// and evaluating the text does not run it: `{ read() { return this.value; } }`. An arrow
/// function has no receiver of its own and is looked through.
pub(super) fn receiver_is_nested(
    semantic: &Semantic<'_>,
    node: NodeId,
    span: Span,
    off: usize,
) -> bool {
    semantic
        .nodes()
        .ancestor_kinds(node)
        .find(|kind| match kind {
            AstKind::Function(_) | AstKind::StaticBlock(_) => true,
            AstKind::PropertyDefinition(p) => p
                .value
                .as_ref()
                .is_some_and(|v| v.span().contains_inclusive(span)),
            _ => false,
        })
        .is_some_and(|owner| owner.span().start as usize >= off)
}

/// The text of every instance field initializer of the class whose body is `body`, however it
/// is laid out across lines or terminated: `metadata =` on one line and its value on the next
/// is one initializer. Fields of a class nested in one of them are that class's. `None` when
/// the body does not parse.
pub(super) fn instance_initializers(body: &str) -> Option<Vec<String>> {
    let wrapped = format!("class __C {{\n{body}\n}}\n");
    for source_type in super::js_code::source_types() {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &wrapped, source_type).parse();
        if parsed.panicked || !parsed.diagnostics.is_empty() {
            continue;
        }
        let built = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&parsed.program);
        let nodes = built.semantic.nodes();
        let mut initializers = Vec::new();
        for node in nodes.iter() {
            let AstKind::PropertyDefinition(field) = node.kind() else {
                continue;
            };
            let Some(value) = field.value.as_ref().filter(|_| !field.r#static) else {
                continue;
            };
            let class = nodes
                .ancestor_kinds(node.id())
                .find(|kind| matches!(kind, AstKind::Class(_)));
            if class.is_some_and(|class| class.span().start == 0) {
                let span = value.span();
                initializers.push(wrapped[span.start as usize..span.end as usize].to_string());
            }
        }
        return Some(initializers);
    }
    None
}
