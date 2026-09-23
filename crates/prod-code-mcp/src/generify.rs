//! Making a parameter generic: a concrete type becomes a type parameter with the bound it needs.
//!
//! `fn total(v: &Vec<u32>)` becomes `fn total<T: AsRef<[u32]>>(v: &T)`. The callers do not change —
//! the type argument is inferred from what they pass — but they are checked: a caller passing a type
//! that does not satisfy the bound, or a body that uses something the bound does not promise, is an
//! error in the overlay before anything is written. The bound is the caller's to choose; which trait
//! a function *should* ask for is a design decision, not something the text can say.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Generified {
    pub function: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// Files that call the function, checked against the new signature.
    pub callers_checked: usize,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Generified {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: `{}`\n- now: `{}`\n- {} file(s) that call it checked against the \
             new signature; their calls do not change, the type argument is inferred\n",
            self.function, self.file, self.was, self.now, self.callers_checked
        );
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str(
                "\nthe analyzer rejects the result — the body uses something the bound does not \
                 promise, or a caller passes a type that does not satisfy it or can no longer be \
                 inferred:\n",
            );
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str("\n[applied]\n");
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make this edit\n");
        }
        out
    }
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A parameter's type split into the reference in front of it (`&`, `&mut `, `&'a `, or nothing)
/// and the type itself.
pub fn split_reference(ty: &str) -> (String, String) {
    let ty = ty.trim();
    let Some(rest) = ty.strip_prefix('&') else {
        return (String::new(), ty.to_string());
    };
    let mut prefix = String::from("&");
    let mut rest = rest.trim_start();
    if rest.starts_with('\'') {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        prefix.push_str(&rest[..end]);
        prefix.push(' ');
        rest = rest[end..].trim_start();
    }
    if let Some(after) = rest.strip_prefix("mut ") {
        prefix.push_str("mut ");
        rest = after.trim_start();
    }
    (prefix, rest.to_string())
}

/// The generic parameter list of a function header: the span inside `<…>` after its name, or `None`
/// when it has none. `name_end` is where the name ends.
pub fn generics_span(text: &str, name_end: usize) -> Option<(usize, usize)> {
    let rest = &text[name_end..];
    let lead = rest.len() - rest.trim_start().len();
    if !rest.trim_start().starts_with('<') {
        return None;
    }
    let open = name_end + lead;
    let mut depth = 0i32;
    for (i, c) in text[open..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open + 1, open + i));
                }
            }
            _ => {}
        }
    }
    None
}

/// Makes the parameter `param` of the function declared at `line`:`col` of `file` generic, as a
/// type parameter `type_param` bounded by `bound`.
#[allow(clippy::too_many_arguments)]
pub async fn generify(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    param: &str,
    bound: &str,
    type_param: &str,
    apply: bool,
    force: bool,
) -> Result<Generified> {
    anyhow::ensure!(
        !type_param.is_empty() && type_param.chars().all(is_ident),
        "`{type_param}` is not a type parameter name"
    );
    let bound = bound.trim();
    anyhow::ensure!(
        !bound.is_empty(),
        "give the `bound` the parameter's type must satisfy"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..start].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    let name_end = start + name.len();

    // The parameter, and the span of its type.
    let list = &text[open..close];
    let mut offset = open;
    let mut found = None;
    for part in crate::signature::split_params(list) {
        let at_in = list[offset - open..]
            .find(part.trim())
            .map(|i| offset + i)
            .unwrap_or(offset);
        let (pattern, ty) = part.split_once(':').unwrap_or((part.as_str(), ""));
        let pattern = pattern.trim().trim_start_matches("mut ").trim();
        if pattern == param {
            let ty_start = at_in + part.trim().find(':').map_or(0, |i| i + 1);
            let lead = text[ty_start..].len() - text[ty_start..].trim_start().len();
            let ty_trim = ty.trim();
            found = Some((
                ty_start + lead,
                ty_start + lead + ty_trim.len(),
                ty_trim.to_string(),
            ));
            break;
        }
        offset = at_in + part.trim().len();
    }
    let (ty_start, ty_end, ty) =
        found.with_context(|| format!("`{name}` has no parameter `{param}`"))?;
    let (reference, concrete) = split_reference(&ty);
    anyhow::ensure!(
        !concrete.starts_with("impl ") && !concrete.starts_with("dyn "),
        "`{param}: {ty}` is already abstract"
    );
    let generics = generics_span(&text, name_end);
    if let Some((g_start, g_end)) = generics {
        let existing = &text[g_start..g_end];
        anyhow::ensure!(
            !existing
                .split(|c: char| !is_ident(c))
                .any(|word| word == type_param),
            "`{name}` already has a generic parameter `{type_param}`; pass another `type_param`"
        );
    }

    let was = text[text[..start].rfind("fn").unwrap_or(start)..close + 1].to_string();
    let mut new_text = text.clone();
    new_text.replace_range(ty_start..ty_end, &format!("{reference}{type_param}"));
    match generics {
        Some((_, g_end)) => {
            let existing = text[..g_end].trim_end();
            let sep = if existing.ends_with('<') || existing.ends_with(',') {
                ""
            } else {
                ", "
            };
            new_text.insert_str(g_end, &format!("{sep}{type_param}: {bound}"));
        }
        None => new_text.insert_str(name_end, &format!("<{type_param}: {bound}>")),
    }
    let fn_at = new_text[..start].rfind("fn").unwrap_or(start);
    let now_end = crate::signature::param_span(&new_text, start)
        .map(|(_, _, c)| c + 1)
        .unwrap_or(new_text.len());
    let now = new_text[fn_at..now_end].to_string();

    // Every file that calls it is checked against the new signature.
    let (nl, nc) = crate::signature::line_col_at(&text, start);
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let callers: BTreeSet<PathBuf> = crate::signature::references(remote, root, file, nl, nc)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(p, _, _)| p)
        .filter(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) != canonical)
        .collect();
    let also: Vec<PathBuf> = callers.iter().cloned().collect();
    let reports = crate::diagnostics::validate_texts(
        remote,
        root,
        &[(file.to_path_buf(), new_text.clone())],
        &also,
    )
    .await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
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

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. The body \
             needs more than the bound promises, or a caller no longer satisfies it or can no longer \
             infer its type (an `.into()` that took its target from the old type); choose another \
             bound, fix the caller, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files: std::collections::BTreeMap<PathBuf, String> =
            std::iter::once((file.to_path_buf(), new_text.clone())).collect();
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }

    Ok(Generified {
        function: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        callers_checked: callers.len(),
        rewritten: vec![(file.to_string_lossy().into_owned(), new_text)],
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_kept_in_front_of_the_type_parameter() {
        assert_eq!(
            split_reference("Vec<u32>"),
            (String::new(), "Vec<u32>".to_string())
        );
        assert_eq!(
            split_reference("&Vec<u32>"),
            ("&".to_string(), "Vec<u32>".to_string())
        );
        assert_eq!(
            split_reference("&mut String"),
            ("&mut ".to_string(), "String".to_string())
        );
        assert_eq!(
            split_reference("&'a str"),
            ("&'a ".to_string(), "str".to_string())
        );
        assert_eq!(
            split_reference("&'a mut Buf"),
            ("&'a mut ".to_string(), "Buf".to_string())
        );
    }

    #[test]
    fn the_generic_list_is_found_after_the_name() {
        let t = "fn f<A: Clone, B>(a: A) {}";
        let (s, e) = generics_span(t, 4).unwrap();
        assert_eq!(&t[s..e], "A: Clone, B");
        let t = "fn g<M: Into<Vec<u8>>>(m: M) {}";
        let (s, e) = generics_span(t, 4).unwrap();
        assert_eq!(&t[s..e], "M: Into<Vec<u8>>");
        assert!(generics_span("fn h(x: u8) {}", 4).is_none());
    }

    #[test]
    fn the_report_says_what_the_signature_became_and_who_was_checked() {
        let done = Generified {
            function: "total".into(),
            root: "/root".into(),
            file: "src/lib.rs".into(),
            was: "fn total(v: &Vec<u32>)".into(),
            now: "fn total<T: AsRef<[u32]>>(v: &T)".into(),
            callers_checked: 2,
            rewritten: vec![],
            diagnostics: vec!["no method named `iter` found (src/lib.rs:2:7)".into()],
            applied: false,
        };
        let text = done.render();
        assert!(
            text.contains("now: `fn total<T: AsRef<[u32]>>(v: &T)`"),
            "{text}"
        );
        assert!(text.contains("2 file(s) that call it checked"), "{text}");
        assert!(text.contains("the bound does not"), "{text}");
        assert!(text.contains("nothing was written"), "{text}");
        let mut ok = done.clone();
        ok.diagnostics.clear();
        ok.applied = true;
        assert!(ok.render().contains("0 errors") && ok.render().contains("[applied]"));
    }
}
