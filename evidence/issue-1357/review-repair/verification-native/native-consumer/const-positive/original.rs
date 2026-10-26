macro_rules! fixed { ($ignored:ident) => { [usize; { let K = 1; K }] }; }
struct S<const N: usize, T>([usize; N], T);
impl<const N: usize> S<N, fixed!(M)> {
    fn f<const M: usize>(c: &S<N, fixed!(M)>) -> usize { c.0[0] + c.1[0] + M }
}
fn main() { let s: S<1, [usize; 1]> = S([4], [2]); println!("{}", S::<1, [usize; 1]>::f::<3>(&s)); }
