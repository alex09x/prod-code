/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use crate::detect::markers::{
    CRYSTAL_MARKERS, CSS_MARKERS, CUE_MARKERS, DOCKERFILE_MARKERS, GRAPHQL_MARKERS, HCL_MARKERS,
    HTML_MARKERS, JSON_MARKERS, JSONNET_MARKERS, MARKDOWN_MARKERS, NIX_MARKERS, POWERSHELL_MARKERS,
    PROTOBUF_MARKERS, SQL_MARKERS, STARLARK_MARKERS, SVELTE_MARKERS, TERRAFORM_MARKERS,
    TOML_MARKERS, TYPST_MARKERS, VUE_MARKERS, YAML_MARKERS,
};

/// A SQL project (.sqlfluff, sqlfluff.cfg, .sqls.json, schema.sql, or *.sql) at the root.
pub fn has_sql_project(root: &Path) -> bool {
    if SQL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".sql"))
        })
        .unwrap_or(false)
}

/// A GraphQL project (codegen.yml, .graphqlrc, schema.graphql, or *.graphql) at the root.
pub fn has_graphql_project(root: &Path) -> bool {
    if GRAPHQL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".graphql") || name.ends_with(".gql")
            })
        })
        .unwrap_or(false)
}

/// A Protobuf project (buf.yaml, buf.gen.yaml, .protolint.yaml, or *.proto) at the root.
pub fn has_protobuf_project(root: &Path) -> bool {
    if PROTOBUF_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".proto"))
        })
        .unwrap_or(false)
}

/// A Crystal project (shard.yml, shard.lock, or *.cr) at the root.
pub fn has_crystal_project(root: &Path) -> bool {
    if CRYSTAL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".cr"))
        })
        .unwrap_or(false)
}

/// A Terraform project (main.tf, versions.tf, *.tf, *.tofu) at the root.
pub fn has_terraform_project(root: &Path) -> bool {
    if TERRAFORM_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".tf") || name.ends_with(".tofu")
            })
        })
        .unwrap_or(false)
}

/// A Nix project (flake.nix, default.nix, shell.nix, *.nix) at the root.
pub fn has_nix_project(root: &Path) -> bool {
    if NIX_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".nix"))
        })
        .unwrap_or(false)
}

/// A Markdown project (.marksman.toml, README.md, *.md, *.markdown) at the root.
pub fn has_markdown_project(root: &Path) -> bool {
    if MARKDOWN_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".md") || name.ends_with(".markdown")
            })
        })
        .unwrap_or(false)
}

/// A YAML project (.yamllint, compose.yaml, etc.) at the root.
pub fn has_yaml_project(root: &Path) -> bool {
    YAML_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A TOML project (taplo.toml, *.toml) at the root.
pub fn has_toml_project(root: &Path) -> bool {
    if TOML_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    false
}

/// A JSON project (.jsonlintrc, *.json, *.jsonc) at the root.
pub fn has_json_project(root: &Path) -> bool {
    JSON_MARKERS.iter().any(|m| root.join(m).exists())
}

/// An HTML project (index.html, *.html, *.htm) at the root.
pub fn has_html_project(root: &Path) -> bool {
    if HTML_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".html") || name.ends_with(".htm")
            })
        })
        .unwrap_or(false)
}

/// A CSS project (styles.css, *.css, *.scss, *.less) at the root.
pub fn has_css_project(root: &Path) -> bool {
    if CSS_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".css") || name.ends_with(".scss") || name.ends_with(".less")
            })
        })
        .unwrap_or(false)
}

/// A Dockerfile project (Dockerfile, Containerfile, *.dockerfile) at the root.
pub fn has_dockerfile_project(root: &Path) -> bool {
    if DOCKERFILE_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("Dockerfile")
                    || name.starts_with("Containerfile")
                    || name.ends_with(".dockerfile")
            })
        })
        .unwrap_or(false)
}

/// A Svelte project (svelte.config.js, *.svelte) at the root.
pub fn has_svelte_project(root: &Path) -> bool {
    if SVELTE_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".svelte"))
        })
        .unwrap_or(false)
}

/// A Vue project (vue.config.js, *.vue) at the root.
pub fn has_vue_project(root: &Path) -> bool {
    if VUE_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".vue"))
        })
        .unwrap_or(false)
}

/// A PowerShell project (PSScriptAnalyzerSettings.psd1, *.ps1, *.psm1, *.psd1) at the root.
pub fn has_powershell_project(root: &Path) -> bool {
    if POWERSHELL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".ps1") || name.ends_with(".psm1") || name.ends_with(".psd1")
            })
        })
        .unwrap_or(false)
}

/// A Starlark / Bazel project (BUILD.bazel, WORKSPACE, *.bzl, Tiltfile, etc.) at the root.
pub fn has_starlark_project(root: &Path) -> bool {
    if STARLARK_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".bzl") || name.ends_with(".star")
            })
        })
        .unwrap_or(false)
}

/// An HCL / Terragrunt project (terragrunt.hcl, .tflint.hcl, *.hcl) at the root.
pub fn has_hcl_project(root: &Path) -> bool {
    if HCL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".hcl"))
        })
        .unwrap_or(false)
}

/// A Typst project (typst.toml, *.typ) at the root.
pub fn has_typst_project(root: &Path) -> bool {
    if TYPST_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".typ"))
        })
        .unwrap_or(false)
}

/// A Jsonnet project (jsonnetfile.json, *.jsonnet, *.libsonnet) at the root.
pub fn has_jsonnet_project(root: &Path) -> bool {
    if JSONNET_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".jsonnet") || name.ends_with(".libsonnet")
            })
        })
        .unwrap_or(false)
}

/// A Cue project (cue.mod, *.cue) at the root.
pub fn has_cue_project(root: &Path) -> bool {
    if CUE_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".cue"))
        })
        .unwrap_or(false)
}
