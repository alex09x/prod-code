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
async fn deref_wrapper_keeps_the_qualified_dispatch_and_runtime_value() {
    let source = "struct Tax;\nstruct S;\nstruct Wrapper(S);\nimpl S {\n    fn f(s: &S) -> u32 { let _ = s; 7 }\n}\nimpl Wrapper { fn f(&self) -> u32 { 99 } }\nimpl std::ops::Deref for Wrapper { type Target = S; fn deref(&self) -> &S { &self.0 } }\nfn main() { let wrapper = &Wrapper(S); println!(\"{}\", S::f(wrapper)); }\n";
    assert_eq!(run(source, "dispatch original"), "7\n");
    let ws = workspace(source);
    let remote = gateway(
        &ws,
        source,
        &[("s: &S", &["s; 7"]), ("f(s:", &["f(wrapper)"])],
    )
    .await;
    let plan = convert(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert!(plan.unmatched.is_empty());
    assert_eq!(run(&plan.rewritten[0].1, "dispatch preview"), "7\n");
    let result = convert(&ws, remote, true, false).await.unwrap();
    assert!(result.applied);
    assert_eq!(run(&ws.read("src/main.rs"), "dispatch applied"), "7\n");
}

#[tokio::test]
async fn full_owner_paths_and_generic_arguments_are_required() {
    for source in [
        "struct Tax;\nstruct S;\nmod other { pub struct S; }\nimpl S {\n    fn f(s: &other::S) -> u32 { let _ = s; 7 }\n}\nfn main() { println!(\"{}\", S::f(&other::S)); }\n",
        "struct Tax;\nstruct Wrapper<T>(T);\nimpl<T> Wrapper<T> {\n    fn f(s: &Wrapper<u32>) -> u32 { s.0 }\n}\nfn main() { println!(\"{}\", Wrapper::<u8>::f(&Wrapper(7))); }\n",
    ] {
        assert_eq!(run(source, "wrong owner original"), "7\n");
        let ws = workspace(source);
        let remote = gateway(&ws, source, &[]).await;
        for force in [false, true] {
            ws.write("src/main.rs", source);
            let result = convert(&ws, remote, true, force).await;
            assert!(
                result.is_err(),
                "accepted a different self type: {result:?}"
            );
            untouched(&ws, source);
        }
    }
}

#[tokio::test]
async fn exact_generic_and_path_owners_still_convert_and_run() {
    for source in [
        "struct Tax;\nstruct S(u32);\nimpl S {\n    fn f(s: &S) -> u32 { s.0 }\n}\nfn main() { println!(\"{}\", S::f(&S(7))); }\n",
        "struct Tax;\nstruct Wrapper<T>(T);\nimpl<T: Copy> Wrapper<T> {\n    fn f(s: &Wrapper<T>) -> T { s.0 }\n}\nfn main() { println!(\"{}\", Wrapper::f(&Wrapper(7))); }\n",
        "struct Tax;\nmod model { pub struct S(pub u32); }\nimpl model::S {\n    fn f(s: &model::S) -> u32 { s.0 }\n}\nfn main() { println!(\"{}\", model::S::f(&model::S(7))); }\n",
    ] {
        let ws = workspace(source);
        let remote = gateway(&ws, source, &[("s: &", &["s.0"]), ("f(s:", &["f(&"])]).await;
        assert_eq!(run(source, "exact owner original"), "7\n");
        let result = convert(&ws, remote, true, false).await.unwrap();
        assert!(result.applied);
        assert_eq!(result.renamed_uses, 1);
        assert_eq!(run(&ws.read("src/main.rs"), "exact owner applied"), "7\n");
    }
}
