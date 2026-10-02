/// The middle order statistic of three signed 32-bit integers.
pub fn median3(a: i32, b: i32, c: i32) -> i32 {
    let mut values = [a, b, c];
    values.sort_unstable();
    values[1]
}
