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
async fn c_family_resolved_typedef_auto_and_ordinary_scopes() {
    for (ext, decl) in [
        ("c", "int"),
        ("c", "Count"),
        ("cpp", "auto"),
        ("cpp", "Count"),
    ] {
        let source = format!(
            "#include <stdio.h>\ntypedef int Count;\nint target(Count x) {{\nint a = x + 1; int b = 0; {{ {decl} x = 10; b = x+1; }} return a+b+(x+1);\n}}\nint main() {{ int x = 4; printf(\"%d\\n\", target(x)); }}\n"
        );
        println!("SOURCE resolved {ext} {decl}\n{source}");
        assert_eq!(run(&source, ext), "21\n");
        let outer = source.find("x)").unwrap();
        let inner = source.find("x = 10").unwrap();
        let first = source.find("x + 1").unwrap();
        let uses = source
            .match_indices("x+1")
            .map(|(n, _)| n)
            .collect::<Vec<_>>();
        let definitions = [(first, outer), (uses[0], inner), (uses[1], outer)];
        let ws = workspace(&source, ext);
        let plan = change_resolved(
            &ws,
            &source,
            ext,
            "select:x + 1",
            true,
            true,
            false,
            false,
            "int",
            &definitions,
        )
        .await
        .unwrap();
        let generated = ws.read(&format!("main.{ext}"));
        println!(
            "GENERATED resolved {ext} replaced={}\n{generated}",
            plan.replaced
        );
        assert_eq!(plan.replaced, 2);
        assert!(generated.contains("b = x+1"));
        assert_eq!(run(&generated, ext), "21\n");
        let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
        assert_ne!(custom, generated);
        println!("CUSTOM resolved {ext}\n{custom}");
        assert_eq!(run(&custom, ext), "51\n");
    }
}

#[tokio::test]
async fn c_family_unknown_identity_refuses_before_any_write() {
    for ext in ["c", "cpp"] {
        let source = "int target(int x) {\nint a = x + 1; return a+(x+1);\n}\nint main() { int x = 4; return target(x) != 10; }\n";
        assert_eq!(run(source, ext), "");
        for known_selection in [false, true] {
            let definitions = if known_selection {
                vec![(source.find("x + 1").unwrap(), source.find("x)").unwrap())]
            } else {
                Vec::new()
            };
            for all in [false, true] {
                if known_selection && !all {
                    continue;
                }
                for (apply, force) in [(false, false), (true, false), (true, true)] {
                    let ws = workspace(source, ext);
                    let error = change_resolved(
                        &ws,
                        source,
                        ext,
                        "select:x + 1",
                        all,
                        apply,
                        force,
                        false,
                        "int",
                        &definitions,
                    )
                    .await
                    .unwrap_err();
                    println!(
                        "REFUSED unknown {ext} known_selection={known_selection} all={all} apply={apply} force={force}: {error}"
                    );
                    assert!(
                        error
                            .to_string()
                            .contains("incomplete C/C++ binding coverage")
                    );
                    assert_eq!(ws.read(&format!("main.{ext}")), source);
                }
            }
        }
        let literal = "#include <stdio.h>\nint target(void) {\nreturn 80+80;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
        let ws = workspace(literal, ext);
        let plan = change_typed(&ws, literal, ext, "80+", true, true, false, false, "int")
            .await
            .unwrap();
        assert_eq!(plan.replaced, 2);
        let generated = ws.read(&format!("main.{ext}"));
        println!("GENERATED literal {ext}\n{generated}");
        assert_eq!(run(&generated, ext), "160\n");
        let custom = generated.replace("target(80)", "target(10)");
        assert_ne!(custom, generated);
        println!("CUSTOM literal {ext}\n{custom}");
        assert_eq!(run(&custom, ext), "20\n");
    }
}

#[tokio::test]
async fn conditional_values_preserve_keys_labels_types_and_nested_branches() {
    for ext in ["js", "ts", "c", "cpp"] {
        let (source, ty) = if ext == "js" {
            (
                "function target() {\nconst a = 80; const b = true ? (false ? 0 : 80) : 0; const o = {80: 1}; again: { break again; } return a+b+o[80];\n}\nconsole.log(target());\n",
                "number",
            )
        } else if ext == "ts" {
            (
                "function target() {\nconst a = 80; const b = true ? (false ? 0 : 80) : 0; const o: Record<number, number> = {80: 1}; again: { break again; } return a+b+o[80];\n}\nconsole.log(target());\n",
                "number",
            )
        } else {
            (
                "#include <stdio.h>\nint target(void) {\nint a = 80; int b = 1 ? (0 ? 0 : 80) : 0; switch (0) { case 80: return 0; default: break; } again: return a+b+1;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n",
                "int",
            )
        };
        assert_eq!(run(source, ext), "161\n");
        let ws = workspace(source, ext);
        let plan = change_typed(&ws, source, ext, "80;", true, true, false, false, ty)
            .await
            .unwrap();
        let generated = ws.read(&format!("main.{ext}"));
        println!(
            "GENERATED conditional {ext} replaced={}\n{generated}",
            plan.replaced
        );
        assert_eq!(run(&generated, ext), "161\n");
        let custom = generated.replace("target(80)", "target(10)");
        assert_ne!(custom, generated);
        println!("CUSTOM conditional {ext}\n{custom}");
        // JS computed index is an independent value; changing it changes lookup.
        if matches!(ext, "js" | "ts") {
            assert_eq!(plan.replaced, 3);
            assert!(generated.contains("{80: 1}"));
            assert_eq!(run(&custom, ext), "NaN\n");
        } else {
            assert_eq!(plan.replaced, 2);
            assert!(generated.contains("case 80:"));
            assert_eq!(run(&custom, ext), "21\n");
        }
        let ws = workspace(source, ext);
        let error = change_typed(
            &ws,
            source,
            ext,
            "select:again",
            false,
            true,
            true,
            false,
            ty,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("complete executable expression"),
            "{ext}: {error}"
        );
        assert_eq!(ws.read(&format!("main.{ext}")), source);
    }
}
