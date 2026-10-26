/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! C-family lexing that a parameter extraction relies on (#1408): a `//` comment continued by
//! a backslash-newline keeps its next physical line, and `++`/`--` are single tokens whose
//! inner sign is never an expression boundary. Programs are compiled and run before and after.

use super::support::*;

#[tokio::test]
async fn a_continued_line_comment_keeps_its_next_line() {
    for (ext, newline) in [("c", "\n"), ("cpp", "\n"), ("c", "\r\n"), ("cpp", "\r\n")] {
        let source = "#include <stdio.h>\nint target(void) {\nint a=80; // keep \\\n80\nreturn a+80;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n"
            .replace('\n', newline);
        let comment = format!("// keep \\{newline}80{newline}");
        println!("SOURCE continued comment {ext} {newline:?}\n{source}");
        assert_eq!(run(&source, ext), "160\n");

        let ws = workspace(&source, ext);
        let plan = change_typed(&ws, &source, ext, "80;", true, true, false, false, "int")
            .await
            .unwrap();
        let generated = &plan.rewritten[0].1;
        println!(
            "GENERATED continued comment replaced={}\n{generated}",
            plan.replaced
        );
        assert_eq!(plan.replaced, 2, "the comment's 80 is not a use");
        assert!(generated.contains(&comment), "{generated}");
        assert_eq!(run(generated, ext), "160\n");
        let custom = generated.replace("target(80)", "target(10)");
        assert_ne!(&custom, generated);
        assert_eq!(run(&custom, ext), "20\n");

        // Selecting the comment's own `80` is refused before anything is written.
        let ws = workspace(&source, ext);
        let needle = format!("80{newline}return");
        let refused = change_typed(&ws, &source, ext, &needle, true, true, false, false, "int")
            .await
            .expect_err("comment data is not an expression");
        println!("REFUSED comment selection: {refused:#}");
        assert_eq!(
            std::fs::read_to_string(ws.path(&format!("main.{ext}"))).unwrap(),
            source
        );
    }
}

#[tokio::test]
async fn the_inner_sign_of_an_increment_is_not_an_expression() {
    for (ext, op) in [("c", "++"), ("cpp", "++"), ("c", "--"), ("cpp", "--")] {
        let source = format!(
            "#include <stdio.h>\nint value=80;\nint target(void) {{\nreturn {op}value;\n}}\nint main(void) {{ int got=target(); printf(\"%d %d\\n\", got, value); }}\n"
        );
        let expected = if op == "++" { "81 81\n" } else { "79 79\n" };
        println!("SOURCE increment {ext} {op}\n{source}");
        assert_eq!(run(&source, ext), expected);

        // Line 4, columns 9..15: the second sign and `value`, whose declaration resolves.
        let inner = format!("select:{}value", &op[1..]);
        let definitions = [(
            source.find(&format!("{op}value")).unwrap() + 2,
            source.find("value=").unwrap(),
        )];
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, ext);
            let refused = change_resolved(
                &ws,
                &source,
                ext,
                &inner,
                false,
                apply,
                force,
                false,
                "int",
                &definitions,
            )
            .await
            .expect_err("half an increment is not an operand");
            println!("REFUSED {ext} {op} apply={apply} force={force}: {refused:#}");
            assert!(
                !format!("{refused:#}").contains("binding coverage"),
                "{refused:#}"
            );
            assert_eq!(
                std::fs::read_to_string(ws.path(&format!("main.{ext}"))).unwrap(),
                source
            );
        }
    }
}
