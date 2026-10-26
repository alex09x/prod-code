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

#[tokio::test]
async fn static_assert_branches_stay_constant() {
    let mut failures = Vec::new();
    for (ext, assertion) in [
        ("cpp", "static_assert(true ? 80 : 0);"),
        (
            "cpp",
            "static_assert((true ? (false ? 0 : 80) : 0), \"constant\");",
        ),
        ("c", "_Static_assert(1 ? 80 : 0, \"constant\");"),
        ("c", "static_assert(1 ? 80 : 0, \"constant\");"),
    ] {
        let source = format!(
            "#include <stdio.h>\n#include <assert.h>\nint target(void) {{\nint a = 80; {assertion} return a;\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE assertion {ext}\n{source}");
        assert_eq!(run(&source, ext), "80\n");
        // Model clangd's macro hover explicitly, independently of the compiler.
        let hovers = if ext == "c" && assertion.starts_with("static_assert") {
            vec![(
                source.find("static_assert(").unwrap(),
                "### macro static_assert\n\n```c\n#define static_assert _Static_assert\n```",
            )]
        } else {
            vec![]
        };
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, ext);
            let plan = change_analyzed(
                &ws,
                &source,
                ext,
                "80;",
                true,
                apply,
                force,
                false,
                "int",
                &[],
                &hovers,
            )
            .await
            .unwrap();
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace("target(80)", "target(10)");
            assert_ne!(&custom, generated);
            println!(
                "GENERATED assertion {ext} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM assertion\n{custom}",
                plan.replaced
            );
            // Record actual compiler diagnostics for both argument choices on a
            // broken pre-repair plan, then continue through every fixture.
            let default = std::panic::catch_unwind(|| run(generated, ext));
            let customized = std::panic::catch_unwind(|| run(&custom, ext));
            if plan.replaced != 1
                || !generated.contains(assertion)
                || !default.as_ref().is_ok_and(|s| s == "80\n")
                || !customized.as_ref().is_ok_and(|s| s == "10\n")
            {
                failures.push(format!(
                    "{ext} {assertion} apply={apply} force={force}: replaced={}",
                    plan.replaced
                ));
            }
        }
        // A selection inside the assertion is never a runtime parameter, even
        // if the caller forces diagnostics. Refuse before modifying the file.
        for force in [false, true] {
            let ws = workspace(&source, ext);
            let needle = if assertion.contains("80)") {
                "80)"
            } else {
                "80 :"
            };
            let result = change_analyzed(
                &ws,
                &source,
                ext,
                needle,
                false,
                true,
                force,
                false,
                "int",
                &[],
                &hovers,
            )
            .await;
            match result {
                Err(e) => {
                    println!("REFUSED assertion {ext} force={force}: {e}");
                    assert!(e.to_string().contains("complete executable expression"));
                    assert_eq!(ws.read(&format!("main.{ext}")), source);
                }
                Ok(_) => failures.push(format!(
                    "{ext}: selected assertion branch accepted force={force}"
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn keyword_member_satisfies_operands_stay_types() {
    let mut failures = Vec::new();
    for name in ["do", "else", "return"] {
        for access in [
            format!("o.{name}"),
            format!("o?.{name}"),
            format!("this.#{name}"),
        ] {
            let source = format!(
                "class C {{\n#{name} = 0x50 as const;\ntarget() {{\nconst a = 80; const o = {{{name}: 0x50 as const}};\n{access} satisfies 80; return a;\n}}\n}}\nconsole.log(new C().target());\n"
            );
            println!("SOURCE member type\n{source}");
            assert_eq!(run(&source, "ts"), "80\n");
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, "ts");
                let plan = change_typed(
                    &ws, &source, "ts", "80;", true, apply, force, false, "number",
                )
                .await
                .unwrap();
                let generated = &plan.rewritten[0].1;
                let custom = generated.replace(".target(80)", ".target(10)");
                assert_ne!(&custom, generated);
                println!(
                    "GENERATED member type apply={apply} force={force} replaced={}\n{generated}\nCUSTOM member type\n{custom}",
                    plan.replaced
                );
                let default = std::panic::catch_unwind(|| run(generated, "ts"));
                let customized = std::panic::catch_unwind(|| run(&custom, "ts"));
                if plan.replaced != 1
                    || !generated.contains(&format!("{access} satisfies 80"))
                    || !default.as_ref().is_ok_and(|s| s == "80\n")
                    || !customized.as_ref().is_ok_and(|s| s == "10\n")
                {
                    failures.push(format!(
                        "{access} apply={apply} force={force}: replaced={}",
                        plan.replaced
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
