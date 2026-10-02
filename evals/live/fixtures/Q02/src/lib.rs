/// Clamp value to the inclusive range; callers guarantee low <= high.
pub fn clamp(value: i32, low: i32, high: i32) -> i32 {
    if value < low { high } else if value > high { low } else { value }
}
