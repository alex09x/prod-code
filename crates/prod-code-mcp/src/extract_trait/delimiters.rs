/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Delimiters in a Rust type/header. An array's element is a type, but everything
/// after its semicolon is a const expression. Parentheses alone do not change
/// that context: tuples and function types may contain nested generic types.
#[derive(Default)]
pub(crate) struct Delimiters {
    angle: usize,
    groups: Vec<Group>,
}

struct Group {
    open: u8,
    expression: bool,
}

impl Delimiters {
    pub(crate) fn top_level(&self) -> bool {
        self.angle == 0 && self.groups.is_empty()
    }

    pub(crate) fn advance(&mut self, text: &str, at: usize) -> Option<()> {
        let c = text.as_bytes()[at];
        let expression = self.groups.iter().any(|group| group.expression);
        match c {
            b'(' | b'[' | b'{' => self.groups.push(Group {
                open: c,
                expression: c == b'{',
            }),
            b')' | b']' | b'}' => {
                let expected = match c {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                if self.groups.pop()?.open != expected {
                    return None;
                }
            }
            b';' => {
                if let Some(group) = self.groups.last_mut()
                    && group.open == b'['
                {
                    group.expression = true;
                }
            }
            b'<' if !expression => self.angle += 1,
            b'>' if !expression && !text[..at].ends_with('-') => {
                self.angle = self.angle.checked_sub(1)?;
            }
            _ => {}
        }
        Some(())
    }
}
