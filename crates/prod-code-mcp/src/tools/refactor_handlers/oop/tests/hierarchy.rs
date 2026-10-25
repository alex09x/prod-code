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

const PULL: &str =
    "class Base:\n    pass\n\nclass Child(Base):\n    def run(self):\n        return 7\n";
const PUSH: &str =
    "class Base:\n    def run(self):\n        return 7\n\nclass Child(Base):\n    pass\n";

#[tokio::test]
async fn pull_and_push_forward_aliases_filtered_members_and_options() {
    for push in [false, true] {
        for (custom, reject) in [(false, false), (true, false), (true, true)] {
            let source = if push { PUSH } else { PULL };
            let ws = Workspace::new(&[("classes.py", source)]);
            let root = ws.root();
            let file = ws.path("classes.py");
            let gw = gateway(reject).await;
            let class = if push { "Base" } else { "Child" };
            let mut args = json!({"path": "classes.py", "members": ["run", null, 3, false]});
            args[if custom { "class_name" } else { "symbol" }] = json!(class);
            if custom {
                args["symbol"] = json!("Ignored");
                args["target_class"] = json!("Base");
                args["target_classes"] = json!([null, "Child", 42]);
                args["clean_siblings"] = json!(false);
                args["force"] = json!(true);
                args["verify"] = json!("none");
            }
            let expected = if push {
                crate::pull_push::push_down_impl(
                    gw.addr(),
                    &root,
                    &file,
                    class,
                    custom.then_some(["Child".to_string()].as_slice()),
                    &["run".to_string()],
                    false,
                    custom,
                    custom.then_some("none"),
                )
                .await
                .unwrap()
            } else {
                crate::pull_push::pull_up_impl(
                    gw.addr(),
                    &root,
                    &file,
                    class,
                    custom.then_some("Base"),
                    &["run".to_string()],
                    !custom,
                    false,
                    custom,
                    custom.then_some("none"),
                )
                .await
                .unwrap()
            };
            assert_eq!(expected.source_class, class);
            assert_eq!(
                expected.target_classes,
                [if push { "Child" } else { "Base" }]
            );
            assert_eq!(expected.members, ["run"]);
            assert_eq!(expected.files_modified, ["classes.py"]);
            assert!(!expected.applied);
            assert_eq!(expected.diagnostics.is_empty(), !reject);
            let response = if push {
                handle_push_down(gw.addr(), &root, &args).await.unwrap()
            } else {
                handle_pull_up(gw.addr(), &root, &args).await.unwrap()
            };
            assert_response(response, expected.render(2048), reject);
            assert_eq!(ws.read("classes.py"), source);
        }
    }
}

#[tokio::test]
async fn empty_or_malformed_members_are_refused_before_reading_the_file() {
    let remote = "127.0.0.1:1".parse().unwrap();
    for members in [Value::Null, json!([]), json!([null, 7]), json!("run")] {
        let args = json!({"path": "missing.py", "class_name": "Child", "members": members});
        assert_eq!(
            handle_pull_up(remote, Path::new("."), &args)
                .await
                .unwrap_err()
                .to_string(),
            "No members specified to pull up"
        );
        assert_eq!(
            handle_push_down(remote, Path::new("."), &args)
                .await
                .unwrap_err()
                .to_string(),
            "No members specified to push down"
        );
    }
}

#[tokio::test]
async fn delegation_forwards_all_three_class_aliases_and_method_filtering() {
    for key in ["sub_type", "symbol", "class_name"] {
        for reject in [false, true] {
            let ws = Workspace::new(&[("classes.py", PUSH)]);
            let root = ws.root();
            let file = ws.path("classes.py");
            let gw = gateway(reject).await;
            let mut args = json!({"path": "classes.py"});
            args[key] = json!("Child");
            let custom = key == "sub_type";
            if custom {
                args["symbol"] = json!("Ignored");
                args["class_name"] = json!("AlsoIgnored");
                args["base_type"] = json!("Base");
                args["field_name"] = json!("delegate");
                args["methods"] = json!(["run", null, 9]);
                args["force"] = json!(true);
                args["verify"] = json!("none");
            }
            let expected = crate::replace_inheritance::replace_inheritance_impl(
                gw.addr(),
                &root,
                &file,
                "Child",
                custom.then_some("Base"),
                custom.then_some("delegate"),
                custom.then_some(["run".to_string()].as_slice()),
                false,
                custom,
                custom.then_some("none"),
            )
            .await
            .unwrap();
            assert_eq!(expected.sub_type, "Child");
            assert_eq!(expected.base_type, "Base");
            assert_eq!(
                expected.field_name,
                if custom { "delegate" } else { "_base" }
            );
            assert_eq!(expected.forwarded_methods, ["run"]);
            assert_eq!(expected.files_modified, ["classes.py"]);
            assert!(!expected.applied);
            assert_eq!(expected.diagnostics.is_empty(), !reject);
            let response = handle_replace_inheritance_with_delegation(gw.addr(), &root, &args)
                .await
                .unwrap();
            assert_response(response, expected.render(2048), reject);
            assert_eq!(ws.read("classes.py"), PUSH);
        }
    }
}

#[tokio::test]
async fn hierarchy_apply_writes_exactly_the_preview() {
    for operation in 0..3 {
        let source = if operation == 0 { PULL } else { PUSH };
        let ws = Workspace::new(&[("classes.py", source)]);
        let root = ws.root();
        let file = ws.path("classes.py");
        let gw = gateway(false).await;
        let (args, expected) = match operation {
            0 => (
                json!({"path": "classes.py", "symbol": "Child", "members": ["run"], "apply": true}),
                crate::pull_push::pull_up_impl(
                    gw.addr(),
                    &root,
                    &file,
                    "Child",
                    None,
                    &["run".into()],
                    true,
                    false,
                    false,
                    None,
                )
                .await
                .unwrap()
                .overlays,
            ),
            1 => (
                json!({"path": "classes.py", "symbol": "Base", "members": ["run"], "apply": true}),
                crate::pull_push::push_down_impl(
                    gw.addr(),
                    &root,
                    &file,
                    "Base",
                    None,
                    &["run".into()],
                    false,
                    false,
                    None,
                )
                .await
                .unwrap()
                .overlays,
            ),
            _ => (
                json!({"path": "classes.py", "symbol": "Child", "apply": true}),
                crate::replace_inheritance::replace_inheritance_impl(
                    gw.addr(),
                    &root,
                    &file,
                    "Child",
                    None,
                    None,
                    None,
                    false,
                    false,
                    None,
                )
                .await
                .unwrap()
                .overlays,
            ),
        };
        let response = match operation {
            0 => handle_pull_up(gw.addr(), &root, &args).await,
            1 => handle_push_down(gw.addr(), &root, &args).await,
            _ => handle_replace_inheritance_with_delegation(gw.addr(), &root, &args).await,
        }
        .unwrap();
        assert!(!response.is_error);
        assert_eq!(ws.read("classes.py"), expected[0].1);
    }
}
