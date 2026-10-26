struct Config(u32);
struct Holder<T>(T);
impl Holder<Config> { fn f(h: &Holder<Config>) -> u32 { (h.0).0 + 1 } }
fn main() { let a = Holder(Config(6)); let b: Holder<Config> = Holder(Config(40)); println!("{} {}", Holder::f(&a), Holder::f(&b)); }
