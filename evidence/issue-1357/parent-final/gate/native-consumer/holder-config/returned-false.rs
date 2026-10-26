struct Config(u32);
struct Holder<T>(T);
impl Holder<Config> { fn f(&self) -> u32 { (self.0).0 + 1 } }
fn main() { let a = Holder(Config(6)); let b: Holder<Config> = Holder(Config(40)); println!("{} {}", a.f(), b.f()); }
