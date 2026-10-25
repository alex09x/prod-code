/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub const QUALIFIED: &str = r#"trait Flag<A, B> { const READY: bool; }
impl Flag<u8, u16> for () { const READY: bool = true; }
pub fn is_ready(n: i32) -> bool {
    match n {
        -1 => return !<() as Flag<u8,u16>>::READY,
        0 => return false == /* operand */ <() as Flag<u8, u16>>::READY,
        1 => return n < 2,
        _ => return <() as Flag<u8, u16>>::READY,
    }
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const OPERAND: &str = r#"struct Builder;
macro_rules! builder {
    ($trait:ident, $method:ident) => {
        impl<F: Fn() -> bool> std::ops::$trait<F> for Builder {
            type Output = F;
            fn $method(self, callback: F) -> F { callback }
        }
    };
}
builder!(Add, add);
builder!(Sub, sub);
builder!(Mul, mul);
builder!(Div, div);
builder!(Rem, rem);
builder!(BitXor, bitxor);
builder!(BitAnd, bitand);
builder!(BitOr, bitor);
builder!(Shl, shl);
builder!(Shr, shr);
pub fn is_ready(n: i32) -> bool {
    let callback = Builder + || { return n >= 0; };
    let sub = Builder - || { return n >= 0; };
    let mul = Builder * || { return n >= 0; };
    let div = Builder / || { return n >= 0; };
    let rem = Builder % || { return n >= 0; };
    let xor = Builder ^ || { return n >= 0; };
    let and = Builder & || { return n >= 0; };
    let or = Builder | || { return n >= 0; };
    let shl = Builder << || { return n >= 0; };
    let shr = Builder >> || { return n >= 0; };
    let typed = [|x: i32| { return x >= 0; }];
    println!("operands={} {} {} {} {} {} {} {} {} {} {}", callback(), sub(),
        mul(), div(), rem(), xor(), and(), or(), shl(), shr(), typed[0](n));
    if n == 0 { return callback(); }
    callback()
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const FUNCTION_MACRO: &str = r#"macro_rules! flag_type { ($($token:tt)*) => { bool }; }
pub fn is_ready(n: i32) -> bool {
    fn helper(n: i32) -> flag_type!{} { return n >= 0; }
    fn nested(
        n: i32,
    ) -> flag_type!{ ([{ ignored }]) }
    { return n >= 0; }
    fn parentheses(n: i32) -> flag_type!() { return n >= 0; }
    fn brackets(n: i32) -> flag_type![] { return n >= 0; }
    println!("helpers={} {} {} {}", helper(n), nested(n), parentheses(n), brackets(n));
    if n == 0 { return helper(n); }
    helper(n)
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const CLOSURE_MACRO: &str = r#"macro_rules! flag_type { ($($token:tt)*) => { bool }; }
pub fn is_ready(n: i32) -> bool {
    let callback = || -> flag_type!{} { return n >= 0; };
    let nested = |x: i32|
        -> flag_type /* 🟦 type trivia */ ! { ([{ ignored }]) }
    { return x >= 0; };
    let parentheses = || -> flag_type!() { return n >= 0; };
    let brackets = || -> flag_type![] { return n >= 0; };
    println!("closures={} {} {} {}", callback(), nested(n), parentheses(), brackets());
    if n == 0 { return callback(); }
    callback()
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;

pub const GENERIC_PIPE: &str = r#"struct S<const N: usize>;
impl<const N: usize> std::ops::BitOr<bool> for S<N> {
    type Output = bool;
    fn bitor(self, rhs: bool) -> bool { rhs }
}
impl<const N: usize, F: Fn() -> bool> std::ops::Shr<F> for S<N> {
    type Output = F;
    fn shr(self, callback: F) -> F { callback }
}
pub fn is_ready(n: i32) -> bool {
    let callback = S::<{ 4 }> >> || { return n >= 0; };
    println!("generic shift={}", callback());
    let typed = |values: &mut dyn Iterator<Item = i32>| { return values.next().is_some(); };
    println!("associated binding={}", typed(&mut [n].into_iter()));
    let _ = S::<1> /* completed expression */ | {
        if n == 0 { return true; }
        false
    };
    let _ = false | false;
    let _ = S::<{ 2 }> | { if n < 0 { return false; } false } | false;
    let _ = S::<3> | { if n > 0 { return false; } false };
    let _ = false || false;
    false
}
pub fn callers(n: i32) -> (bool, bool, String) {
    (is_ready(n), !is_ready(n), is_ready(n).to_string())
}
"#;
