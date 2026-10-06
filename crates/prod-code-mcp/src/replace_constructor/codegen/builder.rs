/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::StructDecl;

/// Generates the fluent builder code to add to the target file.
pub fn generate_builder_code(decl: &StructDecl, builder_name: &str) -> String {
    let name = &decl.name;
    match decl.language.as_str() {
        "rust" => {
            let vis = if decl.is_pub { "pub " } else { "" };
            let generics_header = decl
                .generics
                .as_deref()
                .map(|g| format!("{g} "))
                .unwrap_or_default();
            let generics_name = decl.generics.as_deref().unwrap_or_default();

            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    {}: Option<{}>,", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    {vis}fn {}(mut self, value: {}) -> Self {{\n        self.{} = Some(value);\n        self\n    }}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_fields = decl
                .fields
                .iter()
                .map(|f| {
                    format!(
                        "            {}: self.{}.expect(\"{} is required\"),",
                        f.name, f.name, f.name
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "\n\n#[derive(Default)]\n{vis}struct {builder_name}{generics_name} {{\n{builder_fields}\n}}\n\nimpl {generics_header}{builder_name}{generics_name} {{\n    {vis}fn new() -> Self {{\n        Self::default()\n    }}\n\n{setters_text}\n\n    {vis}fn build(self) -> {name}{generics_name} {{\n        {name} {{\n{build_fields}\n        }}\n    }}\n}}\n\nimpl {generics_header}{name}{generics_name} {{\n    {vis}fn builder() -> {builder_name}{generics_name} {{\n        {builder_name}::default()\n    }}\n}}"
            )
        }
        "go" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    {} {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "func (b *{builder_name}) {}(v {}) *{builder_name} {{\n    b.{} = v\n    return b\n}}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_fields = decl
                .fields
                .iter()
                .map(|f| format!("        {}: b.{},", f.name, f.name))
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "\n\ntype {builder_name} struct {{\n{builder_fields}\n}}\n\nfunc New{builder_name}() *{builder_name} {{\n    return &{builder_name}{{}}\n}}\n\n{setters_text}\n\nfunc (b *{builder_name}) Build() *{name} {{\n    return &{name}{{\n{build_fields}\n    }}\n}}"
            )
        }
        "typescript" | "typescriptreact" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    private _{}?: {};", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    {}(value: {}): this {{\n        this._{} = value;\n        return this;\n    }}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("this._{}!", f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nexport class {builder_name} {{\n{builder_fields}\n\n{setters_text}\n\n    build(): {name} {{\n        return new {name}({build_args});\n    }}\n}}\n"
            )
        }
        "javascript" | "javascriptreact" => {
            let initializers = decl
                .fields
                .iter()
                .map(|field| format!("        this._{} = undefined;", field.name))
                .collect::<Vec<_>>()
                .join("\n");
            let mut setters = Vec::new();
            for field in &decl.fields {
                setters.push(format!(
                    "    {}(value) {{\n        this._{} = value;\n        return this;\n    }}",
                    field.name, field.name
                ));
            }
            let setters_text = setters.join("\n\n");
            let build_args = decl
                .fields
                .iter()
                .map(|field| format!("this._{}", field.name))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n\nexport class {builder_name} {{\n    constructor() {{\n{initializers}\n    }}\n\n{setters_text}\n\n    build() {{\n        return new {name}({build_args});\n    }}\n}}\n"
            )
        }
        "python" => {
            let inits = decl
                .fields
                .iter()
                .map(|f| format!("        self._{} = None", f.name))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    def {}(self, value):\n        self._{} = value\n        return self",
                    f.name, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("{}=self._{}", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nclass {builder_name}:\n    def __init__(self):\n{inits}\n\n{setters_text}\n\n    def build(self) -> \"{name}\":\n        return {name}({build_args})\n"
            )
        }
        "cpp" | "c" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    {} {}_;", f.ty, f.name))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    {builder_name}& {}({} val) {{\n        {}_ = val;\n        return *this;\n    }}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("{}_", f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nstruct {builder_name} {{\n{builder_fields}\n\n{setters_text}\n\n    {name} build() {{\n        return {name}{{{build_args}}};\n    }}\n}};\n"
            )
        }
        "swift" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    private var {}: {}?", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    func set{}(_ value: {}) -> Self {{\n        self.{} = value\n        return self\n    }}",
                    capitalize(&f.name), f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}!", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nclass {builder_name} {{\n{builder_fields}\n\n{setters_text}\n\n    func build() -> {name} {{\n        return {name}({build_args})\n    }}\n}}\n"
            )
        }
        _ => String::new(),
    }
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |f| {
        f.to_uppercase().collect::<String>() + chars.as_str()
    })
}
