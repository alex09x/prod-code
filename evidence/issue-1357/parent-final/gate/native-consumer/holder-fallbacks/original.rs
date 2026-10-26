pub struct Config(pub u32);
pub struct Holder<T>(pub T);
impl Holder<Config> { pub fn f(h: &Holder<Config>) -> u32 { (h.0).0 + 1 } }
trait Intercept { fn f(&self) -> u32 { 999 } }
mod shadow_head {
    pub struct Holder<T>(pub crate::Holder<T>);
    impl<T> Holder<T> { pub fn f(&self) -> u32 { 888 } }
    impl<T> std::ops::Deref for Holder<T> {
        type Target = crate::Holder<T>;
        fn deref(&self) -> &Self::Target { &self.0 }
    }
}
mod other { pub struct Config(pub crate::Holder<crate::Config>); }
impl Intercept for Holder<other::Config> {}
impl std::ops::Deref for Holder<other::Config> {
    type Target = Holder<Config>;
    fn deref(&self) -> &Self::Target { &(self.0).0 }
}
fn main() {
    let exact = Holder(Config(6));
    let head_val = {
        use shadow_head::Holder;
        let bad_head: Holder<Config> = Holder(crate::Holder(Config(10)));
        let _ = bad_head.f();
        crate::Holder::f(&bad_head)
    };
    let arg_val = {
        use other::Config;
        let bad_arg: Holder<Config> = Holder(Config(Holder(crate::Config(20))));
        let _ = bad_arg.f();
        crate::Holder::f(&bad_arg)
    };
    println!("{} {} {}", Holder::f(&exact), head_val, arg_val);
}
