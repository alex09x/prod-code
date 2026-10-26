struct S(u32);
impl S { fn f(s: &S) -> u32 { s.0 + 1 } }
fn main() { let a = S(6); let b = S(40); println!("{} {}", S::f(&a), S::f(&b)); }
