/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// The accessor methods, indented to sit inside an `impl` block at `indent`.
pub fn accessors(
    indent: &str,
    vis: &str,
    name: &str,
    ty: &str,
    by_value: bool,
    setter: bool,
) -> String {
    let (ret, body) = if by_value {
        (ty.to_string(), format!("self.{name}"))
    } else {
        (format!("&{ty}"), format!("&self.{name}"))
    };
    let mut out =
        format!("{indent}{vis}fn {name}(&self) -> {ret} {{\n{indent}    {body}\n{indent}}}\n");
    if setter {
        out.push_str(&format!(
            "\n{indent}{vis}fn set_{name}(&mut self, {name}: {ty}) {{\n{indent}    self.{name} \
             = {name};\n{indent}}}\n"
        ));
    }
    out
}
