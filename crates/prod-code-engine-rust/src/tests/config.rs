/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for configuration loading, deserialization, and CargoConfig mapping.

use ra_ap_project_model::{CargoFeatures, RustLibSource};

use super::create_test_fixture;
use crate::config::{ProcMacroServerKind, ProdCodeConfig};
use crate::engine::RustEngine;
use crate::load_budget;

#[test]
fn prod_code_toml_maps_to_cargo_config() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(ProdCodeConfig::load(temp.path()), ProdCodeConfig::default());
    let defaults = ProdCodeConfig::default().cargo_config();
    assert!(defaults.all_targets);
    assert_eq!(defaults.sysroot, Some(RustLibSource::Discover));
    std::fs::write(
        temp.path().join("prod-code.toml"),
        "[rust]\nfeatures = \"all\"\nsysroot = false\n",
    )
    .unwrap();
    let cfg = ProdCodeConfig::load(temp.path()).cargo_config();
    assert_eq!(cfg.features, CargoFeatures::All);
    assert_eq!(cfg.sysroot, None);
    std::fs::write(
        temp.path().join("prod-code.toml"),
        "[rust]\nfeatures = [\"a\", \"b\"]\nno_default_features = true\n",
    )
    .unwrap();
    let cfg = ProdCodeConfig::load(temp.path()).cargo_config();
    assert_eq!(
        cfg.features,
        CargoFeatures::Selected {
            features: vec!["a".into(), "b".into()],
            no_default_features: true
        }
    );
}

#[test]
fn a_cold_load_limits_cargo_jobs_even_when_the_project_requests_more() {
    let (temp, _) = create_test_fixture();
    std::fs::create_dir(temp.path().join(".cargo")).unwrap();
    std::fs::write(
        temp.path().join(".cargo/config.toml"),
        "[build]\njobs = 64\n",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("build.rs"),
        r#"
fn main() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let jobs = std::env::var("NUM_JOBS").unwrap();
    std::fs::write(std::path::Path::new(&root).join("jobs.txt"), jobs).unwrap();
}
"#,
    )
    .unwrap();
    let _engine = RustEngine::load(temp.path()).unwrap();
    let jobs: usize = std::fs::read_to_string(temp.path().join("jobs.txt"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(jobs, load_budget::shared().workers);
}

#[test]
fn test_rust_analysis_options_proc_macro_deserialization() {
    let toml_str = r#"
[rust]
build_scripts = true
proc_macro_srv = "sandboxed"
proc_macro_workers = 3
proc_macro_memory_limit_mb = 1024
"#;
    let config: ProdCodeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.rust.proc_macro_srv, ProcMacroServerKind::Sandboxed);
    assert_eq!(config.rust.proc_macro_workers, Some(3));
    assert_eq!(config.rust.proc_macro_memory_limit_mb, Some(1024));

    let sysroot_toml = r#"
[rust]
proc_macro_srv = "sysroot"
"#;
    let config: ProdCodeConfig = toml::from_str(sysroot_toml).unwrap();
    assert_eq!(config.rust.proc_macro_srv, ProcMacroServerKind::Sysroot);

    let disabled_toml = r#"
[rust]
proc_macro_srv = "disabled"
"#;
    let config: ProdCodeConfig = toml::from_str(disabled_toml).unwrap();
    assert_eq!(config.rust.proc_macro_srv, ProcMacroServerKind::Disabled);
}
