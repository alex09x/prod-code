fn compute(w: u32) -> u32 {
    let cached = w + 1;
    let a = cached;
    let b = cached;
    a + b
}
fn main() {
    for input in [0, 7, 100] { println!("{}", compute(input)); }
}
