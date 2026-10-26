macro_rules! identity { ($ty:ty) => { ($ty, [u8; { let A = 1; A }]) }; }
trait T { fn value(&self)->u8; }
struct A {}
impl T for A { fn value(&self)->u8 {2} }
impl A { fn value(&self)->u8 {1} }
struct S<U>(U);
impl S<identity!(A)> { fn f<A:T>(c:&S<identity!(A)>)->u8 {c.0.0.value()} }
fn main(){println!("{}",S::<(A,[u8;1])>::f::<A>(&S((A {},[7]))));}
