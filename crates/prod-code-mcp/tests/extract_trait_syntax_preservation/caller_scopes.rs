/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{run_mcp_tool_extract, scripted_gateway, workspace};
use prod_code_testkit::answers;

const LIB: &str = "pub struct Rect;\n\
impl Rect { pub fn area(&self) -> u32 { 1 } }\n\
mod child {\n\
    use super::Rect;\n\
    pub fn run() -> u32 { Rect.area() }\n\
}\n";

const EXAMPLE: &str = "use shape_api::Rect;\n\
\n\
fn main() {\n\
    assert_eq!(Rect.area(), 1);\n\
}\n";

const BENCH: &str = "use shape_api::Rect;\n\
\n\
fn main() {\n\
    assert_eq!(Rect.area(), 1);\n\
}\n";

const SAME_NAME_TEST: &str = "use shape_api::Rect;\n\
pub fn test_caller() -> u32 { Rect.area() }\n";

const CARGO: &str = "[package]\n\
name = \"shape-api\"\n\
version = \"0.1.0\"\n\
edition = \"2021\"\n\
[[example]]\n\
name = \"demo\"\n\
path = \"examples/demo.rs\"\n\
test = true\n";
const BENCH_TARGET: &str = "[[bench]]\n\
name = \"demo-bench\"\n\
path = \"benches/demo.rs\"\n\
harness = true\n";

fn reference_locations(
    lib: &std::path::Path,
    example: &std::path::Path,
    bench: &std::path::Path,
    test: &std::path::Path,
) -> serde_json::Value {
    let mut locations = answers::locations(lib, &[(5, 32)]);
    locations
        .as_array_mut()
        .expect("LSP locations are an array")
        .extend(
            answers::locations(example, &[(4, 21)])
                .as_array()
                .expect("LSP locations are an array")
                .iter()
                .cloned(),
        );
    locations
        .as_array_mut()
        .expect("LSP locations are an array")
        .extend(
            answers::locations(test, &[(2, 36)])
                .as_array()
                .expect("LSP locations are an array")
                .iter()
                .cloned(),
        );
    locations
        .as_array_mut()
        .expect("LSP locations are an array")
        .extend(
            answers::locations(bench, &[(4, 21)])
                .as_array()
                .expect("LSP locations are an array")
                .iter()
                .cloned(),
        );
    locations
}

pub async fn same_file_inline_module_and_example_targets_get_their_trait_imports() {
    let manifest = format!("{CARGO}{BENCH_TARGET}");
    let files = [
        ("Cargo.toml", manifest.as_str()),
        ("src/lib.rs", LIB),
        ("examples/demo.rs", EXAMPLE),
        ("benches/demo.rs", BENCH),
        ("tests/shape_api.rs", SAME_NAME_TEST),
    ];
    let ws = workspace(&files);
    let lib = ws.path("src/lib.rs");
    let example = ws.path("examples/demo.rs");
    let bench = ws.path("benches/demo.rs");
    let same_name_test = ws.path("tests/shape_api.rs");
    let refs = reference_locations(&lib, &example, &bench, &same_name_test);
    let gateway = scripted_gateway(refs, answers::no_diagnostics()).await;

    let preview = run_mcp_tool_extract(
        gateway.addr(),
        &ws,
        "src/lib.rs",
        2,
        1,
        &["area"],
        "Measure",
        false,
        false,
        false,
    )
    .await
    .expect("public preview");

    assert!(
        preview.contains("src/lib.rs: added `use super::Measure;`"),
        "inline child module import missing from public preview: {preview}"
    );
    assert!(
        preview.contains("examples/demo.rs: added `use shape_api::Measure;`"),
        "example target import must use the library crate name: {preview}"
    );
    assert!(
        preview.contains("benches/demo.rs: added `use shape_api::Measure;`"),
        "benchmark target import must use the library crate name: {preview}"
    );
    assert!(
        preview.contains("tests/shape_api.rs: added `use shape_api::Measure;`"),
        "a test target sharing the library's crate name must still use the library path: {preview}"
    );
    assert_eq!(ws.read("src/lib.rs"), LIB, "preview must not write");
    assert_eq!(
        ws.read("examples/demo.rs"),
        EXAMPLE,
        "preview must not write"
    );
    assert_eq!(ws.read("benches/demo.rs"), BENCH, "preview must not write");
    assert_eq!(
        ws.read("tests/shape_api.rs"),
        SAME_NAME_TEST,
        "preview must not write"
    );

    let apply_ws = workspace(&files);
    let apply_lib = apply_ws.path("src/lib.rs");
    let apply_example = apply_ws.path("examples/demo.rs");
    let apply_bench = apply_ws.path("benches/demo.rs");
    let apply_test = apply_ws.path("tests/shape_api.rs");
    let refs = reference_locations(&apply_lib, &apply_example, &apply_bench, &apply_test);
    let gateway = scripted_gateway(refs, answers::no_diagnostics()).await;
    let applied = run_mcp_tool_extract(
        gateway.addr(),
        &apply_ws,
        "src/lib.rs",
        2,
        1,
        &["area"],
        "Measure",
        false,
        true,
        false,
    )
    .await
    .expect("public apply");

    assert!(applied.contains("[applied]"), "{applied}");
    let lib_after = apply_ws.read("src/lib.rs");
    let example_after = apply_ws.read("examples/demo.rs");
    let bench_after = apply_ws.read("benches/demo.rs");
    let same_name_test_after = apply_ws.read("tests/shape_api.rs");
    assert!(lib_after.contains("use super::Measure;"), "{lib_after}");
    assert!(
        example_after.contains("use shape_api::Measure;"),
        "{example_after}"
    );
    assert!(
        bench_after.contains("use shape_api::Measure;"),
        "{bench_after}"
    );
    assert!(
        same_name_test_after.contains("use shape_api::Measure;"),
        "{same_name_test_after}"
    );

    let cargo_test = std::process::Command::new("cargo")
        .args(["test", "--all-targets"])
        .current_dir(apply_ws.root())
        .output()
        .expect("run the complete fixture's Cargo targets");
    assert!(
        cargo_test.status.success(),
        "generated trait imports did not compile:\n{}\n{}",
        String::from_utf8_lossy(&cargo_test.stdout),
        String::from_utf8_lossy(&cargo_test.stderr)
    );
}
