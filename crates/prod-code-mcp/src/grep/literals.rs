/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Blanking the text of string and character literals, line by line, for the enclosing-symbol
//! scan: braces and keywords inside a literal are text, not code. The quotes stay, every other
//! character inside becomes a space, so columns and line lengths are unchanged.

/// The open literal a line ends inside, carried to the next line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    /// `"…"`, with backslash escapes; Swift's `"""` opens and closes as three of these.
    Quoted,
    /// Rust `r"…"` / `r#"…"#`: no escapes, closed by `"` and this many `#`.
    Raw(usize),
    /// `` `…` ``: Go raw strings and JavaScript templates.
    Backtick,
}

/// `lines` with the inside of every string and character literal blanked.
pub(crate) fn mask_literals(lines: &[&str]) -> Vec<String> {
    let mut open: Option<Open> = None;
    lines
        .iter()
        .map(|line| mask_line(line, &mut open))
        .collect()
}

fn mask_line(line: &str, open: &mut Option<Open>) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match *open {
            Some(Open::Quoted) => {
                if c == '\\' {
                    out.push(' ');
                    if i + 1 < chars.len() {
                        out.push(' ');
                    }
                    i += 2;
                    continue;
                }
                if c == '"' {
                    *open = None;
                    out.push('"');
                } else {
                    out.push(' ');
                }
            }
            Some(Open::Raw(hashes)) => {
                if c == '"'
                    && chars[i + 1..]
                        .iter()
                        .take(hashes)
                        .filter(|&&h| h == '#')
                        .count()
                        == hashes
                {
                    *open = None;
                    out.push('"');
                    out.extend(std::iter::repeat_n('#', hashes));
                    i += 1 + hashes;
                    continue;
                }
                out.push(' ');
            }
            Some(Open::Backtick) => {
                if c == '`' {
                    *open = None;
                    out.push('`');
                } else {
                    out.push(' ');
                }
            }
            None => {
                // A line comment ends the code on this line; its text is not scanned for
                // literals (an apostrophe in a comment opens nothing).
                if c == '/' && chars.get(i + 1) == Some(&'/') {
                    out.extend(&chars[i..]);
                    break;
                }
                if c == 'r' && !is_ident(i.checked_sub(1).map(|p| chars[p])) {
                    let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
                    if chars.get(i + 1 + hashes) == Some(&'"') {
                        out.push('r');
                        out.extend(std::iter::repeat_n('#', hashes));
                        out.push('"');
                        *open = Some(Open::Raw(hashes));
                        i += 2 + hashes;
                        continue;
                    }
                }
                match c {
                    '"' => {
                        *open = Some(Open::Quoted);
                        out.push('"');
                    }
                    '`' => {
                        *open = Some(Open::Backtick);
                        out.push('`');
                    }
                    '\'' => {
                        // A character literal (`'{'`, `'\n'`); a lifetime (`'a`) has no closing
                        // quote and is left as it is.
                        let len = match chars.get(i + 1) {
                            Some('\\') => chars[i + 2..]
                                .iter()
                                .position(|&q| q == '\'')
                                .map(|p| p + 3),
                            Some(_) if chars.get(i + 2) == Some(&'\'') => Some(3),
                            _ => None,
                        };
                        match len {
                            Some(len) if i + len <= chars.len() => {
                                out.push('\'');
                                out.extend(std::iter::repeat_n(' ', len - 2));
                                out.push('\'');
                                i += len;
                                continue;
                            }
                            _ => out.push('\''),
                        }
                    }
                    _ => out.push(c),
                }
            }
        }
        i += 1;
    }
    out
}

fn is_ident(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::mask_literals;

    /// The text inside a literal of length `n` once blanked.
    fn blank(n: usize) -> String {
        " ".repeat(n)
    }

    #[test]
    fn literal_text_is_blanked_and_code_is_kept() {
        let lines = [
            r#"    let s = "final class View: NSView {";"#,
            r#"fn f() { let c = '{'; let l: &'a str = x; }"#,
            r##"let raw = r#"impl Foo { "quoted" }"#; g()"##,
            r#"let multi = "line one {"#,
            r#"line two }"; h()"#,
            r#"let e = "a \" b {"; k()"#,
            "x := `{`",
            "// a comment's { brace",
        ];
        let masked = mask_literals(&lines);
        let inside = |s: &str| s.chars().count();
        assert_eq!(
            masked[0],
            format!(
                "    let s = \"{}\";",
                blank(inside("final class View: NSView {"))
            )
        );
        assert_eq!(masked[1], "fn f() { let c = ' '; let l: &'a str = x; }");
        assert_eq!(
            masked[2],
            format!(
                "let raw = r#\"{}\"#; g()",
                blank(inside(r#"impl Foo { "quoted" }"#))
            )
        );
        assert_eq!(
            masked[3],
            format!("let multi = \"{}", blank(inside("line one {")))
        );
        assert_eq!(masked[4], format!("{}\"; h()", blank(inside("line two }"))));
        assert_eq!(
            masked[5],
            format!("let e = \"{}\"; k()", blank(inside(r#"a \" b {"#)))
        );
        assert_eq!(masked[6], "x := ` `");
        assert_eq!(masked[7], "// a comment's { brace");
        for (line, before) in masked.iter().zip(lines) {
            assert_eq!(line.chars().count(), before.chars().count(), "{line}");
        }
    }
}
