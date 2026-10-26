const N: usize = 1;
struct S<const K: usize>(usize);
impl S<N> { fn f<const N: usize>(c: &S<N>) -> usize { c.0 + N } }
fn main() { println!("{}", S::<N>::f::<2>(&S::<2>(5))); }
