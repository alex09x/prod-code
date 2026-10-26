/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

pub(crate) type Edit = (usize, usize, String);
pub(crate) struct Setter {
    pub at: usize,
    pub start: usize,
    pub end: usize,
    pub method: String,
    pub receiver: Option<(usize, crate::parameter_object::Language)>,
}

impl Setter {
    pub fn new(
        text: &str,
        at: usize,
        start: usize,
        end: usize,
        method: String,
        lang: crate::parameter_object::Language,
    ) -> Self {
        use crate::parameter_object::Language;
        let receiver =
            matches!(lang, Language::Rust | Language::Python | Language::Cpp).then(|| {
                let before = text[..at].trim_end();
                let dot = before.len() - if before.ends_with("->") { 2 } else { 1 };
                (super::chain_start(text, dot), lang)
            });
        Self {
            at,
            start,
            end,
            method,
            receiver,
        }
    }

    fn begin(&self) -> usize {
        self.receiver.map_or(self.at, |(at, _)| at)
    }
}

/// Compose the innermost RHS first. All spans remain in the original coordinate space;
/// replacing a setter consumes its child edits rather than applying them a second time.
pub(crate) fn compose(text: &str, edits: &mut Vec<Edit>, mut setters: Vec<Setter>) -> Result<()> {
    use crate::parameter_object::Language;
    setters.sort_by_key(|s| s.end - s.begin());
    for setter in setters {
        let Setter {
            at,
            start,
            end,
            ref method,
            receiver,
        } = setter;
        let from = setter.begin();
        let mut children = Vec::new();
        let mut target_children = Vec::new();
        for edit in edits.iter() {
            if edit.0 >= start && edit.0 + edit.1 <= end {
                children.push((edit.0 - start, edit.1, edit.2.clone()));
            } else if edit.0 >= from && edit.0 + edit.1 <= at {
                target_children.push((edit.0 - from, edit.1, edit.2.clone()));
            } else {
                anyhow::ensure!(
                    edit.0 + edit.1 <= from || edit.0 >= end,
                    "overlapping field edits cannot be composed; nothing was written"
                );
            }
        }
        let rhs = apply(&text[start..end], children)?;
        // A newline terminating // is syntax, not disposable whitespace.
        let rhs = rhs.trim_start().trim_end_matches([' ', '\t', '\r']);
        let target = apply(&text[from..at], target_children)?;
        let mut value = "prod_code_field_value".to_string();
        while text.contains(&value) || method.contains(&value) {
            value.push('_');
        }
        let replacement = match receiver.map(|(_, lang)| lang) {
            // A plain place and a literal or path value have no evaluation order to keep.
            // C++ keeps the long form: a setter call would change where conversions run.
            Some(Language::Rust | Language::Python)
                if plain_place(&target) && plain_value(rhs, &target) =>
            {
                format!("{}{method}({rhs})", target.trim_start())
            }
            Some(Language::Rust) => {
                let prefix = text[..from].trim_end();
                anyhow::ensure!(
                    target.trim_end().strip_suffix('.').is_some_and(|receiver| {
                        !receiver.trim().is_empty()
                            && !prefix.ends_with(['!', '#'])
                            && (!prefix.ends_with('>') || prefix.ends_with("=>"))
                    }),
                    "unsupported Rust assignment receiver; nothing was written"
                );
                // Rust checks the assignment place before its RHS, but evaluates
                // the RHS first. The empty Option's place supplies the setter's
                // contextual type without any path bypassing RHS initialization.
                // Its witness is evaluated only after RHS, even when RHS initializes
                // a local used by the target. None means the witness never calls it.
                // Diverge after the witness so a consuming receiver is moved only
                // on a path that cannot reach the actual setter. This branch remains
                // impossible at runtime: the Option is still None until assignment.
                // The match keeps RHS temporaries alive through the real setter.
                // No declaration-file type names escape into the consumer module.
                format!(
                    "{{ let mut {value} = ::core::option::Option::None; match *{{ if let ::core::option::Option::Some({value}) = {value}.take() {{ {target}{method}({value}); loop {{}} }} &mut {value} }} = ::core::option::Option::Some({rhs}) {{ () => {{ {target}{method}({value}.unwrap()); }} }} }}"
                )
            }
            Some(Language::Python) => {
                let indent = super::super::polyglot::python_rhs::logical_indent(text, from);
                // A final base-indented pass keeps any original semicolon suffix
                // outside the finally suite. Cleanup also runs when the target or
                // setter raises, including when the caller retains its traceback.
                format!(
                    "{value} = ({rhs})\n{indent}try:\n{indent}    {target}{method}({value})\n{indent}finally:\n{indent}    del {value}\n{indent}pass"
                )
            }
            Some(Language::Cpp) => {
                let getter = method.replacen("set_", "get_", 1);
                let thunk = format!("{value}_target");
                let (receiver, access) = target
                    .trim_end()
                    .strip_suffix("->")
                    .map(|receiver| (receiver, "->"))
                    .or_else(|| target.trim_end().strip_suffix('.').map(|r| (r, ".")))
                    .ok_or_else(|| anyhow::anyhow!("unsupported C++ assignment receiver"))?;
                // Deduce the setter parameter type in a dead conditional branch so lambda
                // receivers stay out of C++17 unevaluated operands and structured-binding
                // receivers stay outside every lambda body. C++17 assignment sequences the
                // RHS conversion before evaluating the receiver on the LHS.
                format!(
                    "([](auto&& {thunk}) {{ struct {thunk}_setter {{ decltype(({thunk})) target; void operator=(const decltype({thunk}{access}{getter}())& {value}) && {{ static_cast<decltype({thunk})&&>(target){access}{method}({value}); }} }}; return {thunk}_setter{{{thunk}}}; }}(({receiver}))) = ([](auto {thunk}_type) {{ return [](const decltype({thunk}_type())& {value}) -> const decltype({thunk}_type())& {{ return {value}; }}; }}(true ? nullptr : ([](auto&& {thunk}) -> decltype({thunk}{access}{getter}()) (*)() {{ return nullptr; }}(({receiver})))))({rhs})"
                )
            }
            _ => format!("{method}({rhs})"),
        };
        edits.retain(|(pos, len, _)| !(*pos >= from && pos + len <= end));
        edits.push((from, end - from, replacement));
    }
    Ok(())
}

/// `cfg.`, `self.inner.` or `this->`: a chain of names ending at the field's receiver dot.
fn plain_place(target: &str) -> bool {
    let target = target.trim().replace("->", ".");
    let Some(chain) = target.strip_suffix('.') else {
        return false;
    };
    !chain.is_empty()
        && chain
            .split('.')
            .all(|name| !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
}

/// A literal number or boolean, or a path such as `limit` or `Mode::FAST`, that does not
/// read through the receiver being assigned.
fn plain_value(rhs: &str, target: &str) -> bool {
    let rhs = rhs.trim();
    let body = rhs.strip_prefix('-').unwrap_or(rhs);
    let root = target.trim().split(['.', '-']).next().unwrap_or("");
    !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':'))
        && !body.split(['.', ':']).any(|name| name == root)
}

pub(crate) fn apply(text: &str, mut edits: Vec<Edit>) -> Result<String> {
    edits.sort_by_key(|(at, len, _)| (*at, *len));
    let mut body = text.to_string();
    let mut boundary = text.len();
    for (at, len, replacement) in edits.into_iter().rev() {
        anyhow::ensure!(
            at + len <= boundary && text.is_char_boundary(at) && text.is_char_boundary(at + len),
            "overlapping or invalid field edit; nothing was written"
        );
        body.replace_range(at..at + len, &replacement);
        boundary = at;
    }
    Ok(body)
}
