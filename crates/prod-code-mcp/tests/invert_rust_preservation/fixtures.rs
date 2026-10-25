/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub const RAW_LITERAL: &str =
    "r##\"🟦 embedded \" return true;\n  raw indent and \\\"quoted\\\"\n\"##";
pub const RAW: &str = r####"pub fn is_ready(n: i32) -> bool {
    let raw = r##"🟦 embedded " return true;
  raw indent and \"quoted\"
"##;
    let ordinary = "🟦 escaped \" return false; }";
    let bytes = br#"byte " return false;"#;
    let character = '🟦';
    let shapes = (r"no hashes {return false;}", cr##"C " return true; }"##,
        r###"short "# } return true; "###);
    /* " return true; /* nested comment */ return false; */
    // r#"return true;"#
    println!("raw={raw:?} ordinary={ordinary:?} bytes={bytes:?} char={character}");
    println!("shapes={shapes:?}");
    if n == 0 { return raw.len() > 3; }
    n > 0
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"####;

pub const HELPER: &str = "fn helper(\n        n: i32,\n    )\n        -> bool\n    {\n        if n == 0 { return true; }\n        return n > 0;\n    }";
pub const NESTED: &str = r#"pub fn is_ready(n: i32) -> bool {
    fn helper(
        n: i32,
    )
        -> bool
    {
        if n == 0 { return true; }
        return n > 0;
    }
    let closure = |x: i32|
        -> bool
    {
        return x >= 0;
    };
    let expression_closure = || return n >= 0;
    let nested_closure = || { let inner = || { return n >= 0; }; return inner(); };
    let future = async move
    {
        fn inner(
        ) -> i32
        { return 7; }
        return inner();
    };
    // Execute this immediately-ready future without an executor dependency.
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let value = std::future::Future::poll(future.as_mut(), &mut cx);
    println!("helpers={} {} {} {} {:?}", helper(n), closure(n), expression_closure(), nested_closure(), value);
    if n == 0
    {
        return helper(n);
    }
    return {
        if n < 0 { return false; }
        closure(n)
    };
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const GENERIC: &str = r#"pub fn is_ready<T>(n: i32) -> bool {
    let _ = std::marker::PhantomData::<T>;
    if n == 0 { return true; }
    n > 0
}
pub fn callers(n: i32) -> (bool, bool, String, bool, bool) {
    (is_ready::<Vec<Option<u8>>>(n),
     !is_ready::<fn() -> bool>(n),
     is_ready::<[u8; { 1 << 2 }]>(n) /* boundary */ .to_string(),
     crate::selected::is_ready::<Option<(u8, u8)>>(n),
     is_ready /* comment */ :: <
         fn() -> Vec<Option<u8>>
     > (n))
}
"#;

pub const BORROWED: &str = r#"pub fn is_ready(n: i32) -> bool {
    struct Slot<T> { callback: T }
    let shared = &|| { return n >= 0; };
    let mutable = &mut /* borrowed */ || { return n >= 0; };
    let moved = &mut move || { return n >= 0; };
    let field = Slot { callback: || { return n >= 0; } };
    let borrowed_field = Slot { callback: &|| { return n >= 0; } };
    println!("borrowed={} {} {} {} {}", shared(), mutable(), moved(),
        (field.callback)(), (borrowed_field.callback)());
    if n == 0 { return shared(); }
    shared() && mutable() && moved() && (field.callback)() && (borrowed_field.callback)()
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const OPERATORS: &str = r#"pub fn is_ready(n: i32) -> bool {
    let café = n < 0;
    // These pipes are operators: their RHS returns belong to is_ready.
    if &"🟦" == &r"blue" /* literal operand, not a borrowed closure */
        || { if n < 0 { return false; } false } { return true; }
    if café || { if n == 0 { return true; } false } { return false; }
    if ('🟦' == 'x') /* trailing comment */ || { if n > 0 { return false; } false } {
        return true;
    }
    let _ = false | { if n < -1 { return true; } false };
    n >= 0
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const BREAKS: &str = r#"pub fn is_ready(n: i32) -> bool {
    let plain = loop { break || { return n >= 0; }; };
    let labeled = 'résultat: loop {
        break /* value */ 'résultat /* closure */ || { return n >= 0; };
    };
    let moved = 'outer: loop { break 'outer move || { return n >= 0; }; };
    let typed = loop { break |x: i32|
        -> bool { return x >= 0; }; };
    let expression = loop { break || return n >= 0; };
    println!("breaks={} {} {} {} {}", plain(), labeled(), moved(), typed(n), expression());
    plain() && labeled() && moved() && typed(n) && expression()
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const MATCH_ARMS: &str = r#"pub fn is_ready(n: i32) -> bool {
    fn check<A, B>(n: i32) -> bool { n > 0 }
    match n {
        0 => return true
        , 1 => return check /* generic comma */ :: /* trivia */ <(u8, u8), Vec<u8>>(n),
        _ => { return n < 0 },
    }
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;
