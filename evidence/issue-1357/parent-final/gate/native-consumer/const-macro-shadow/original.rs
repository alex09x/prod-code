macro_rules! cap { ($n:expr) => { [usize; $n] }; }
const N: usize = 1;
struct S<T>(T);
impl S<cap!(N)> { fn f<const N: usize>(c: &S<cap!(N)>) -> usize { c.0[0] + N } }
fn main() { println!("{}", S::<[usize; 1]>::f::<2>(&S([5, 8]))); }
