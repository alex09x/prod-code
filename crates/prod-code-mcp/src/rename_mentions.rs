//! What a rename leaves behind: the old name in comments and in the names of the tests that
//! exercise it. The analyzer renames every reference and nothing else; a doc comment that says
//! "`Order` is written once" and a test called `order_total_rounds` keep the old name.
//!
//! In a comment, the old name is replaced where it stands as a whole word. In the name of a test
//! function (one with `#[test]`, `#[tokio::test]` or another attribute ending in `test`), its
//! snake_case form is replaced where it stands between underscores, so renaming `Order` to
//! `Trade` turns `order_total_rounds` into `trade_total_rounds`.

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `CamelCase` and `snake_case` names in snake_case: `OrderLine` gives `order_line`.
pub fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `text` with every whole-word `old` replaced by `new`, and how many there were.
fn replace_word(text: &str, old: &str, new: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut n = 0;
    let mut at = 0;
    while let Some(i) = text[at..].find(old) {
        let start = at + i;
        let end = start + old.len();
        let whole = !text[..start].chars().next_back().is_some_and(is_ident)
            && !text[end..].chars().next().is_some_and(is_ident);
        out.push_str(&text[at..start]);
        if whole {
            out.push_str(new);
            n += 1;
        } else {
            out.push_str(old);
        }
        at = end;
    }
    out.push_str(&text[at..]);
    (out, n)
}

/// A test name with the snake_case `old` replaced where it stands between underscores (or at an
/// end): `order_total_rounds` with `order` -> `trade` gives `trade_total_rounds`.
pub fn test_name(name: &str, old: &str, new: &str) -> Option<String> {
    let parts: Vec<&str> = name.split('_').collect();
    let olds: Vec<&str> = old.split('_').collect();
    if olds.is_empty() || parts.len() < olds.len() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut hit = false;
    while i < parts.len() {
        if parts[i..].starts_with(&olds) {
            out.push(new.to_string());
            i += olds.len();
            hit = true;
        } else {
            out.push(parts[i].to_string());
            i += 1;
        }
    }
    hit.then(|| out.join("_"))
}

/// What a pass changed in one file.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Mentions {
    pub comments: usize,
    /// Test functions renamed, old name then new.
    pub tests: Vec<(String, String)>,
}

/// Whether the function declared on `lines[at]` is a test: an attribute above it (past doc
/// comments and other attributes) is `#[test]` or ends in `test` or `test(...)`.
fn is_test(lines: &[&str], at: usize) -> bool {
    let mut i = at;
    while i > 0 {
        i -= 1;
        let l = lines[i].trim();
        if l.starts_with("///") || l.starts_with("//") {
            continue;
        }
        let Some(attr) = l.strip_prefix("#[") else {
            return false;
        };
        let path = attr.split(['(', ']']).next().unwrap_or("").trim();
        if path == "test" || path.ends_with("::test") || path.ends_with("_test") || path == "rstest"
        {
            return true;
        }
    }
    false
}

/// `text` with the old name replaced in its comments and in its test names.
pub fn rewrite(text: &str, old: &str, new: &str) -> (String, Mentions) {
    let mut found = Mentions::default();
    let (old_snake, new_snake) = (snake(old), snake(new));
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (n, line) in lines.iter().enumerate() {
        // A comment: `//`, `///`, `//!` after the code on the line, or the whole line.
        if let Some(c) = comment_start(line) {
            let (rest, k) = replace_word(&line[c..], old, new);
            found.comments += k;
            out.push(format!("{}{rest}", &line[..c]));
            continue;
        }
        let mut decl = line.trim_start();
        while let Some(rest) = ["pub ", "pub(crate) ", "async ", "const ", "unsafe "]
            .iter()
            .find_map(|p| decl.strip_prefix(p))
        {
            decl = rest;
        }
        if let Some(rest) = decl.strip_prefix("fn ") {
            let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
            if is_test(&lines, n)
                && let Some(renamed) = test_name(&name, &old_snake, &new_snake)
            {
                let at = line.find(&format!("fn {name}")).unwrap_or(0) + 3;
                found.tests.push((name.clone(), renamed.clone()));
                out.push(format!(
                    "{}{renamed}{}",
                    &line[..at],
                    &line[at + name.len()..]
                ));
                continue;
            }
        }
        out.push(line.to_string());
    }
    (out.join("\n"), found)
}

/// Where the comment on `line` starts, outside a string literal.
fn comment_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i + 1 < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && bytes[i + 1] == b'/' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_test_names_follow_a_rename() {
        let text = "/// An `Order` is written once; `Orders` are not.\npub struct Order;\n\nfn f() -> &'static str {\n    \"// Order in a string\" // Order here\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn order_total_rounds() {}\n\n    /// Doc.\n    #[tokio::test]\n    async fn reorders_order() {}\n\n    fn order_helper() {}\n}\n";
        let (out, found) = rewrite(text, "Order", "Trade");
        assert!(
            out.starts_with("/// An `Trade` is written once; `Orders` are not.\npub struct Order;"),
            "{out}"
        );
        assert!(
            out.contains("\"// Order in a string\" // Trade here"),
            "{out}"
        );
        assert!(out.contains("fn trade_total_rounds()"), "{out}");
        assert!(out.contains("async fn reorders_trade()"), "{out}");
        assert!(out.contains("fn order_helper()"), "not a test: {out}");
        assert_eq!(found.comments, 2);
        assert_eq!(
            found.tests,
            [
                (
                    "order_total_rounds".to_string(),
                    "trade_total_rounds".to_string()
                ),
                ("reorders_order".to_string(), "reorders_trade".to_string()),
            ]
        );
    }

    #[test]
    fn names_are_matched_in_snake_case_and_by_whole_segments() {
        assert_eq!(snake("OrderLine"), "order_line");
        assert_eq!(snake("parse_order"), "parse_order");
        assert_eq!(
            test_name("parses_order_line_twice", "order_line", "trade_line").as_deref(),
            Some("parses_trade_line_twice")
        );
        assert_eq!(test_name("reorder_lines", "order", "trade"), None);
    }
}
