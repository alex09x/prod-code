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
            let (generics_decl, where_clause) = super::factory::rust_declaration_parts(decl);
            let (generics_header, generics_name) =
                super::factory::rust_impl_generics(generics_decl);

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
            let empty_fields = decl
                .fields
                .iter()
                .map(|f| format!("            {}: None,", f.name))
                .collect::<Vec<_>>()
                .join("\n");

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
                "\n\n{vis}struct {builder_name}{generics_decl}{where_clause} {{\n{builder_fields}\n}}\n\nimpl {generics_header}Default for {builder_name}{generics_name}{where_clause} {{\n    fn default() -> Self {{\n        Self {{\n{empty_fields}\n        }}\n    }}\n}}\n\nimpl {generics_header}{builder_name}{generics_name}{where_clause} {{\n    {vis}fn new() -> Self {{\n        Self::default()\n    }}\n\n{setters_text}\n\n    {vis}fn build(self) -> {name}{generics_name} {{\n        {name} {{\n{build_fields}\n        }}\n    }}\n}}\n\nimpl {generics_header}{name}{generics_name}{where_clause} {{\n    {vis}fn builder() -> {builder_name}{generics_name} {{\n        {builder_name}::default()\n    }}\n}}"
            )
        }
        "go" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    _{} {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "func (b *{builder_name}) {}(v {}) *{builder_name} {{\n    b._{} = v\n    return b\n}}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_fields = decl
                .fields
                .iter()
                .map(|f| format!("        {}: b._{},", f.name, f.name))
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "\n\ntype {builder_name} struct {{\n{builder_fields}\n}}\n\nfunc New{builder_name}() *{builder_name} {{\n    return &{builder_name}{{}}\n}}\n\n{setters_text}\n\nfunc (b *{builder_name}) Build() *{name} {{\n    return &{name}{{\n{build_fields}\n    }}\n}}"
            )
        }
        "typescript" | "typescriptreact" => {
            let helper = super::members::argument_helper(decl);
            let storage = super::members::fresh_member(decl, "_arguments");
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| {
                    format!(
                        "    private _{}?: {};",
                        f.name,
                        if f.ty.is_empty() { "any" } else { &f.ty }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    {}(value: {}): this {{\n        this._{} = value;\n        return this;\n    }}",
                    f.name, if f.ty.is_empty() { "any" } else { &f.ty }, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| {
                    if f.vis == "rest" {
                        format!("...(this._{} ?? [])", f.name)
                    } else {
                        format!("this._{}!", f.name)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nexport class {builder_name} {{\n{builder_fields}\n    private {storage}?: any[];\n\n    {helper}(...values: any[]): this {{\n        this.{storage} = values;\n        return this;\n    }}\n\n{setters_text}\n\n    build(): {name} {{\n        if (this.{storage}) return new {name}(...this.{storage} as ConstructorParameters<typeof {name}>);\n        return new {name}({build_args});\n    }}\n}}\n"
            )
        }
        "javascript" | "javascriptreact" => {
            let helper = super::members::argument_helper(decl);
            let storage = super::members::fresh_member(decl, "_arguments");
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
                .map(|field| {
                    if field.vis == "rest" {
                        format!("...(this._{} ?? [])", field.name)
                    } else {
                        format!("this._{}", field.name)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n\nexport class {builder_name} {{\n    constructor() {{\n{initializers}\n    }}\n\n    {helper}(...values) {{\n        this.{storage} = values;\n        return this;\n    }}\n\n{setters_text}\n\n    build() {{\n        if (this.{storage}) return new {name}(...this.{storage});\n        return new {name}({build_args});\n    }}\n}}\n"
            )
        }
        "python" => {
            let optional = decl.fields.iter().any(|f| f.vis == "default");
            let provided_member = super::members::fresh_member(decl, "_provided");
            let inits = decl
                .fields
                .iter()
                .map(|f| format!("        self._{} = None", f.name))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                let provided = if optional {
                    format!("\n        self.{provided_member}[\"{}\"] = True", f.name)
                } else {
                    String::new()
                };
                setters.push(format!(
                    "    def {}(self, value):\n        self._{} = value{provided}\n        return self",
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
            let inits = if optional {
                format!("        self.{provided_member} = {{}}\n{inits}")
            } else if inits.is_empty() {
                "        pass".into()
            } else {
                inits
            };
            let build_args = if optional {
                let saved = decl
                    .fields
                    .iter()
                    .map(|f| format!("(\"{}\", self._{})", f.name, f.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "**{{field: value for field, value in [{saved}] if field in self.{provided_member}}}"
                )
            } else {
                build_args
            };

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
