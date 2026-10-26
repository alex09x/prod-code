/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::Workspace;

#[tokio::test]
async fn python_qualified_and_parenthesized_await_preserves_argument_evaluation() {
    let source = "import asyncio\nclass C:\n    async def f(self, x):\n        return x\nc = C()\ndef receiver():\n    print('receiver')\n    return c\ndef argument():\n    print('argument')\n    return 7\nasync def outer(x):\n    return x\nasync def main():\n    print(await receiver().f(argument()))\n    print(await (c.f(8)))\n    print(await ((c.f(9))))\n    print(await outer(await c.f(10)))\nasyncio.run(main())\n";
    let ws = Workspace::new(&[("main.py", source)]);
    let root = &ws.root();
    evidence(root, "main.py", "original");
    let original = run(root, "python3", &["main.py"]);
    let gw = gateway(
        ["f(argument())", "f(8)", "f(9)", "f(10)"]
            .map(|call| location(root, "main.py", source, call))
            .to_vec(),
    )
    .await;
    change_with(
        gw.addr(),
        root,
        &root.join("main.py"),
        3,
        15,
        &[Param::Keep("x".into())],
        &Modifiers {
            asyncness: Some(false),
            ..Default::default()
        },
        true,
        false,
    )
    .await
    .unwrap();
    evidence(root, "main.py", "generated");
    assert_eq!(run(root, "python3", &["main.py"]), original);
    let generated = std::fs::read_to_string(root.join("main.py")).unwrap();
    assert!(generated.contains("await outer("));
    assert!(generated.contains("receiver().f(argument())"));
}

#[tokio::test]
async fn python_comments_keywords_unbound_receivers_and_outer_awaits() {
    let source = "import asyncio\nclass C:\n    async def f(self, x):\n        return x\nc = C()\nasync def outer(x):\n    return x\nasync def main():\n    print(await ( # keep await comment\n        c.f( x = 7 )\n    ))\n    print(await C.f(c, 8))\n    print(await outer(await (c.f(9))))\n    print(await c.f(8 // 2))\nasyncio.run(main())\n";
    let ws = Workspace::new(&[("main.py", source)]);
    let root = &ws.root();
    let original = run(root, "python3", &["main.py"]);
    let gw = gateway(
        ["f( x = 7 )", "f(c, 8)", "f(9)", "f(8 // 2)"]
            .map(|call| location(root, "main.py", source, call))
            .to_vec(),
    )
    .await;
    change_with(
        gw.addr(),
        root,
        &root.join("main.py"),
        3,
        15,
        &[Param::Keep("x".into())],
        &Modifiers {
            asyncness: Some(false),
            ..Default::default()
        },
        true,
        false,
    )
    .await
    .unwrap();
    let generated = std::fs::read_to_string(root.join("main.py")).unwrap();
    assert!(generated.contains("# keep await comment"));
    assert!(generated.contains("c.f( x = 7 )"));
    assert!(generated.contains("C.f(c, 8)"));
    assert!(generated.contains("c.f(8 // 2)"));
    assert!(generated.contains("await outer((c.f(9)))"));
    assert_eq!(run(root, "python3", &["main.py"]), original);
}

#[tokio::test]
async fn javascript_full_operand_preserves_comments_and_outer_await() {
    let source = "class C {\n    async f(x) { return x; }\n}\nconst c = new C();\nasync function outer(x) { return x; }\nasync function main() {\n    console.log(await /* before receiver */ c.f( 7 ));\n    console.log(await ((c.f(8))));\n    console.log(await outer(await c.f(9)));\n}\nmain();\n";
    let ws = Workspace::new(&[("main.js", source)]);
    let root = &ws.root();
    let original = run(root, "node", &["main.js"]);
    let gw = gateway(
        ["f( 7 )", "f(8)", "f(9)"]
            .map(|call| location(root, "main.js", source, call))
            .to_vec(),
    )
    .await;
    change_with(
        gw.addr(),
        root,
        &root.join("main.js"),
        2,
        11,
        &[Param::Keep("x".into())],
        &Modifiers {
            asyncness: Some(false),
            ..Default::default()
        },
        true,
        false,
    )
    .await
    .unwrap();
    let generated = std::fs::read_to_string(root.join("main.js")).unwrap();
    assert!(generated.contains("/* before receiver */ c.f( 7 )"));
    assert!(generated.contains("console.log(((c.f(8))))"));
    assert!(generated.contains("await outer(c.f(9))"));
    assert_eq!(run(root, "node", &["main.js"]), original);
}

#[tokio::test]
async fn python_async_enable_places_await_before_complete_receiver() {
    let source = "import asyncio\nclass C:\n    def f(self, x):\n        return x\nc = C()\ndef receiver():\n    print('receiver')\n    return c\nasync def main():\n    print(receiver().f( 7 ))\nasyncio.run(main())\n";
    let ws = Workspace::new(&[("main.py", source)]);
    let root = &ws.root();
    let original = run(root, "python3", &["main.py"]);
    let gw = gateway(vec![location(root, "main.py", source, "f( 7 )")]).await;
    change_with(
        gw.addr(),
        root,
        &root.join("main.py"),
        3,
        9,
        &[Param::Keep("x".into())],
        &Modifiers {
            asyncness: Some(true),
            ..Default::default()
        },
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(run(root, "python3", &["main.py"]), original);
    assert!(
        std::fs::read_to_string(root.join("main.py"))
            .unwrap()
            .contains("(await receiver().f( 7 ))")
    );
}
