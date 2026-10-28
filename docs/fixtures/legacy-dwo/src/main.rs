#[inline(never)]
fn work(seed: u64) -> u64 {
    let mut value = seed;
    for step in 1..64 {
        value = value.wrapping_mul(step).wrapping_add(step * 3);
    }
    value
}

fn main() {
    assert_ne!(work(17), 0);
}
