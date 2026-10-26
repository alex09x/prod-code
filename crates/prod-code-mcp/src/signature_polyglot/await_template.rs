/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Template text is opaque, but substitutions use the ordinary expression
/// lexer and its delimiter stack. Each nested template has its own frame.
#[derive(Default)]
pub(super) struct Context {
    frames: Vec<Frame>,
}

struct Frame {
    opening: usize,
    expression: Option<usize>,
}

pub(super) enum Step {
    Text(usize, Option<usize>),
    Expression(usize),
}

impl Context {
    pub fn start(&mut self, opening: usize) {
        self.frames.push(Frame {
            opening,
            expression: None,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn skip(&mut self, content: &str, at: usize, depth: usize) -> Option<Step> {
        let frame = self.frames.last_mut()?;
        if frame
            .expression
            .is_some_and(|expression| depth >= expression)
        {
            return None;
        }
        frame.expression = None;
        let bytes = content.as_bytes();
        let mut end = at;
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => end = (end + 2).min(bytes.len()),
                b'`' => {
                    let opening = self.frames.pop()?.opening;
                    return Some(Step::Text(end + 1, Some(opening)));
                }
                b'$' if bytes.get(end + 1) == Some(&b'{') => {
                    if end != at {
                        return Some(Step::Text(end, None));
                    }
                    frame.expression = Some(depth + 1);
                    return Some(Step::Expression(end + 1));
                }
                _ => end += 1,
            }
        }
        Some(Step::Text(end, None))
    }
}
