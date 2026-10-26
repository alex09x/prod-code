macro_rules! fixed { () => { [u8; { let U = 1; U }] }; }
struct S<T>(T);
impl S<fixed!()> { fn f<U>(&self) -> u8 { self.0[0] } }
fn main() { println!("{}", S::f::<()>(&S([7]))); }
