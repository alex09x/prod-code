/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::parse::*;
use crate::verify::plan::*;
use crate::verify::types::*;

#[test]
fn a_line_of_output_becomes_an_event_as_it_arrives() {
    let json = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":5,"is_primary":true}],"children":[]}}"#;
    assert!(matches!(
        event_of_line("rust", VerifyKind::Check, json),
        Some(RunEvent::Diagnostic(d)) if d.message == "mismatched types"
    ));
    assert_eq!(
        event_of_line("rust", VerifyKind::Test, "test tests::adds ... ok"),
        Some(RunEvent::Test {
            name: "tests::adds".into(),
            ok: true
        })
    );
    assert_eq!(
        event_of_line("rust", VerifyKind::Test, "test tests::fails ... FAILED"),
        Some(RunEvent::Test {
            name: "tests::fails".into(),
            ok: false
        })
    );
    assert_eq!(
        event_of_line("rust", VerifyKind::Test, "test x ... ignored"),
        None
    );
    assert_eq!(
        event_of_line(
            "go",
            VerifyKind::Test,
            r#"{"Action":"fail","Package":"p","Test":"TestX"}"#
        ),
        Some(RunEvent::Test {
            name: "TestX".into(),
            ok: false
        })
    );
    assert_eq!(
        event_of_line("go", VerifyKind::Test, r#"{"Action":"run","Test":"TestX"}"#),
        None
    );
    assert_eq!(event_of_line("python", VerifyKind::Test, "PASSED"), None);
    let text = serde_json::to_string(&RunEvent::Test {
        name: "t".into(),
        ok: true,
    })
    .unwrap();
    assert_eq!(text, r#"{"event":"test","name":"t","ok":true}"#);
}

#[test]
fn test_polyglot_plan_commands() {
    assert_eq!(
        plan_command("haskell", VerifyKind::Check, None).unwrap(),
        ["cabal", "build"]
    );
    assert_eq!(
        plan_command("haskell", VerifyKind::Test, Some("foo")).unwrap(),
        [
            "cabal",
            "test",
            "--test-show-details=direct",
            "--test-option=-mfoo"
        ]
    );
    assert_eq!(
        plan_command("ocaml", VerifyKind::Check, None).unwrap(),
        ["dune", "build"]
    );
    assert_eq!(
        plan_command("ocaml", VerifyKind::Test, None).unwrap(),
        ["dune", "runtest"]
    );
    assert_eq!(
        plan_command("clojure", VerifyKind::Check, None).unwrap(),
        ["lein", "check"]
    );
    assert_eq!(
        plan_command("clojure", VerifyKind::Test, Some("my-ns")).unwrap(),
        ["lein", "test", ":only my-ns"]
    );
    assert_eq!(
        plan_command("julia", VerifyKind::Check, None).unwrap(),
        ["julia", "--project", "-e", "using Pkg; Pkg.build()"]
    );
    assert_eq!(
        plan_command("shell", VerifyKind::Check, None).unwrap(),
        ["shellcheck", "**/*.sh"]
    );
    assert_eq!(
        plan_command("shell", VerifyKind::Test, Some("test/my_test.bats")).unwrap(),
        ["bats", "test", "test/my_test.bats"]
    );
    assert_eq!(
        plan_command("r", VerifyKind::Check, None).unwrap(),
        ["R", "CMD", "check", "."]
    );
    assert_eq!(
        plan_command("erlang", VerifyKind::Check, None).unwrap(),
        ["rebar3", "compile"]
    );
    assert_eq!(
        plan_command("erlang", VerifyKind::Test, Some("my_mod")).unwrap(),
        ["rebar3", "eunit", "--module=my_mod"]
    );
    assert_eq!(
        plan_command("fsharp", VerifyKind::Check, None).unwrap(),
        ["dotnet", "build"]
    );
    assert_eq!(
        plan_command("fsharp", VerifyKind::Test, Some("MyTest")).unwrap(),
        ["dotnet", "test", "--filter", "MyTest"]
    );
    let perl_cmd = plan_command("perl", VerifyKind::Check, None).unwrap();
    assert_eq!(perl_cmd[..2], ["sh", "-c"]);
    assert!(perl_cmd[2].contains("perl -Ilib -c"));
    assert_eq!(
        plan_command("perl", VerifyKind::Test, Some("t/foo.t")).unwrap(),
        ["prove", "-l", "t/foo.t"]
    );
    assert_eq!(
        plan_command("solidity", VerifyKind::Check, None).unwrap(),
        ["forge", "build"]
    );
    assert_eq!(
        plan_command("solidity", VerifyKind::Test, Some("testTransfer")).unwrap(),
        ["forge", "test", "--match-test", "testTransfer"]
    );
    assert_eq!(
        plan_command("nim", VerifyKind::Check, None).unwrap(),
        ["nim", "check"]
    );
    assert_eq!(
        plan_command("nim", VerifyKind::Test, None).unwrap(),
        ["nimble", "test"]
    );
    assert_eq!(
        plan_command("d", VerifyKind::Check, None).unwrap(),
        ["dub", "build"]
    );
    assert_eq!(
        plan_command("d", VerifyKind::Test, Some("unit")).unwrap(),
        ["dub", "test", "--", "unit"]
    );
    assert_eq!(
        plan_command("fortran", VerifyKind::Check, None).unwrap(),
        ["fpm", "build"]
    );
    assert_eq!(
        plan_command("fortran", VerifyKind::Test, Some("my_test")).unwrap(),
        ["fpm", "test", "--target", "my_test"]
    );
    assert_eq!(
        plan_command("sql", VerifyKind::Check, None).unwrap(),
        ["sqlfluff", "lint"]
    );
    assert_eq!(
        plan_command("sql", VerifyKind::Test, Some("t/*.sql")).unwrap(),
        ["pg_prove", "t/*.sql"]
    );
    assert_eq!(
        plan_command("graphql", VerifyKind::Check, None).unwrap(),
        ["graphql-codegen", "--check"]
    );
    assert_eq!(
        plan_command("graphql", VerifyKind::Test, None).unwrap(),
        ["graphql-codegen"]
    );
    assert_eq!(
        plan_command("protobuf", VerifyKind::Check, None).unwrap(),
        ["buf", "lint"]
    );
    assert_eq!(
        plan_command("protobuf", VerifyKind::Test, None).unwrap(),
        ["buf", "breaking", "--against", ".git#branch=main"]
    );
    assert_eq!(
        plan_command("crystal", VerifyKind::Check, None).unwrap(),
        ["crystal", "build", "--no-codegen"]
    );
    assert_eq!(
        plan_command("crystal", VerifyKind::Test, Some("my_spec")).unwrap(),
        ["crystal", "spec", "-e", "my_spec"]
    );
    assert_eq!(
        plan_command("groovy", VerifyKind::Check, None).unwrap(),
        ["gradle", "compileGroovy"]
    );
    assert_eq!(
        plan_command("groovy", VerifyKind::Test, Some("MyTest")).unwrap(),
        ["gradle", "test", "--tests", "MyTest"]
    );
    assert_eq!(
        plan_command("ada", VerifyKind::Check, None).unwrap(),
        ["gprbuild", "-c"]
    );
    assert_eq!(
        plan_command("ada", VerifyKind::Test, Some("test_suite")).unwrap(),
        ["gprtest", "test_suite"]
    );
    assert_eq!(
        plan_command("v", VerifyKind::Check, None).unwrap(),
        ["v", "check", "."]
    );
    assert_eq!(
        plan_command("v", VerifyKind::Test, Some("test_foo")).unwrap(),
        ["v", "test", ".", "test_foo"]
    );
    assert_eq!(
        plan_command("racket", VerifyKind::Check, None).unwrap(),
        ["raco", "make"]
    );
    assert_eq!(
        plan_command("racket", VerifyKind::Test, Some("foo-test.rkt")).unwrap(),
        ["raco", "test", ".", "foo-test.rkt"]
    );
    assert_eq!(
        plan_command("terraform", VerifyKind::Check, None).unwrap(),
        ["terraform", "validate"]
    );
    assert_eq!(
        plan_command("terraform", VerifyKind::Test, Some("tests/unit")).unwrap(),
        ["terraform", "test", "-filter", "tests/unit"]
    );
    assert_eq!(
        plan_command("nix", VerifyKind::Check, None).unwrap(),
        ["nix", "flake", "check"]
    );
    assert_eq!(
        plan_command("markdown", VerifyKind::Check, None).unwrap(),
        ["markdownlint", "."]
    );
    assert_eq!(
        plan_command("yaml", VerifyKind::Check, None).unwrap(),
        ["yamllint", "."]
    );
    assert_eq!(
        plan_command("toml", VerifyKind::Check, None).unwrap(),
        ["taplo", "check"]
    );
    assert_eq!(
        plan_command("json", VerifyKind::Check, None).unwrap(),
        ["jsonlint", "."]
    );
    assert_eq!(
        plan_command("html", VerifyKind::Check, None).unwrap(),
        ["htmlhint", "."]
    );
    assert_eq!(
        plan_command("css", VerifyKind::Check, None).unwrap(),
        ["stylelint", "**/*.css"]
    );
    assert_eq!(
        plan_command("dockerfile", VerifyKind::Check, None).unwrap(),
        ["hadolint", "Dockerfile"]
    );
    assert_eq!(
        plan_command("svelte", VerifyKind::Check, None).unwrap(),
        ["svelte-check"]
    );
    assert_eq!(
        plan_command("vue", VerifyKind::Check, None).unwrap(),
        ["vue-tsc", "--noEmit"]
    );
    let asm_cmd = plan_command("assembly", VerifyKind::Check, None).unwrap();
    assert_eq!(asm_cmd[..2], ["sh", "-c"]);
    assert!(asm_cmd[2].contains("nasm -f elf64"));
}

#[test]
fn test_universal_verification_matrix_plans_supported_languages_and_refuses_targetless_zig() {
    let languages = [
        "rust",
        "go",
        "swift",
        "csharp",
        "java",
        "kotlin",
        "php",
        "ruby",
        "dart",
        "zig",
        "elixir",
        "scala",
        "lua",
        "haskell",
        "ocaml",
        "clojure",
        "julia",
        "shell",
        "r",
        "erlang",
        "fsharp",
        "perl",
        "solidity",
        "nim",
        "d",
        "fortran",
        "sql",
        "graphql",
        "protobuf",
        "crystal",
        "groovy",
        "ada",
        "v",
        "racket",
        "terraform",
        "nix",
        "markdown",
        "yaml",
        "toml",
        "json",
        "html",
        "css",
        "dockerfile",
        "svelte",
        "vue",
        "assembly",
    ];

    for lang in languages {
        let check_cmd = plan_command(lang, VerifyKind::Check, None);
        assert!(
            check_cmd.is_ok(),
            "Language {lang} failed to plan check command: {:?}",
            check_cmd.err()
        );
        let check = check_cmd.unwrap();
        assert!(!check.is_empty(), "Language {lang} check command is empty");

        let test_cmd = plan_command(lang, VerifyKind::Test, None);
        if lang == "zig" {
            let error = test_cmd.expect_err("targetless Zig test planning must fail closed");
            assert!(
                error.to_string().contains("test step"),
                "Zig planning error should explain the missing target: {error}"
            );
            continue;
        }
        assert!(
            test_cmd.is_ok(),
            "Language {lang} failed to plan test command: {:?}",
            test_cmd.err()
        );
        let test = test_cmd.unwrap();
        assert!(!test.is_empty(), "Language {lang} test command is empty");
    }
}
