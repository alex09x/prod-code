/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

const SOURCE: &str = "struct Item { value: i32 }\nfn item() -> Item { Item { value: 7 } }\n";

#[tokio::test]
async fn constructor_handlers_forward_defaults_names_and_diagnostics() {
    for builder in [false, true] {
        for (custom, reject) in [(false, false), (true, false), (true, true)] {
            let ws = Workspace::new(&[("item.rs", SOURCE)]);
            let root = ws.root();
            let file = ws.path("item.rs");
            let gw = gateway(reject).await;
            let mut args = json!({"path": "item.rs", "type_name": "Item"});
            let name = custom.then_some(if builder { "Assembler" } else { "make" });
            if let Some(name) = name {
                args[if builder {
                    "builder_name"
                } else {
                    "factory_name"
                }] = json!(name);
                args["verify"] = json!("none");
                args["apply"] = json!(false);
                args["force"] = json!(true);
            }
            let expected = if builder {
                crate::replace_constructor::replace_constructor_with_builder(
                    gw.addr(),
                    &root,
                    &file,
                    "Item",
                    name,
                    false,
                    custom,
                    custom.then_some("none"),
                )
                .await
                .unwrap()
            } else {
                crate::replace_constructor::replace_constructor_with_factory(
                    gw.addr(),
                    &root,
                    &file,
                    "Item",
                    name,
                    false,
                    custom,
                    custom.then_some("none"),
                )
                .await
                .unwrap()
            };
            assert_eq!(
                expected.target_name,
                name.unwrap_or(if builder { "ItemBuilder" } else { "new" })
            );
            assert_eq!(expected.declared_fields, ["value: i32"]);
            assert_eq!(expected.instantiations_rewritten, 1);
            assert!(!expected.applied);
            assert_eq!(expected.diagnostics.is_empty(), !reject);
            let response = if builder {
                handle_replace_constructor_with_builder(gw.addr(), &root, &args)
                    .await
                    .unwrap()
            } else {
                handle_replace_constructor_with_factory(gw.addr(), &root, &args)
                    .await
                    .unwrap()
            };
            assert_response(response, expected.render(2048), reject);
            assert_eq!(ws.read("item.rs"), SOURCE);
        }
    }
}

#[tokio::test]
async fn constructors_apply_the_exact_preview_and_preserve_absolute_paths() {
    for builder in [false, true] {
        let ws = Workspace::new(&[("item.rs", SOURCE)]);
        let root = ws.root();
        let file = ws.path("item.rs");
        let gw = gateway(false).await;
        let expected = if builder {
            crate::replace_constructor::replace_constructor_with_builder(
                gw.addr(),
                &root,
                &file,
                "Item",
                None,
                false,
                false,
                None,
            )
            .await
            .unwrap()
        } else {
            crate::replace_constructor::replace_constructor_with_factory(
                gw.addr(),
                &root,
                &file,
                "Item",
                None,
                false,
                false,
                None,
            )
            .await
            .unwrap()
        };
        let args = json!({"path": file, "type_name": "Item", "apply": true});
        let result = if builder {
            handle_replace_constructor_with_builder(gw.addr(), &root, &args)
                .await
                .unwrap()
        } else {
            handle_replace_constructor_with_factory(gw.addr(), &root, &args)
                .await
                .unwrap()
        };
        assert!(!result.is_error);
        assert_eq!(expected.rewritten[0].0, file.to_string_lossy());
        assert_eq!(ws.read("item.rs"), expected.rewritten[0].1);
    }
}
