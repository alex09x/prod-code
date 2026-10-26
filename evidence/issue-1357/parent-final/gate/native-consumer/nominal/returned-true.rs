struct S(u32);
impl S { fn f(&self) -> u32 { self.0 + 1 } }
fn main() { let a = S(6); let b = S(40); println!("{} {}", a.f(), b.f()); }
