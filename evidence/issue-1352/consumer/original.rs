fn compute(w: u32) -> u32 {
    let a = w + 1;
    let b = w + 1;
    a + b
}
fn main() {
    for input in [0, 7, 100] { println!("{}", compute(input)); }
}
