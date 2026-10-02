/// Clamp value to the inclusive range; callers guarantee low <= high.
pub fn clamp(value: i32, low: i32, high: i32) -> i32 {
    if value < low { low } else if value > high { high } else { value }
}

#[cfg(test)]
mod tests {
    use super::clamp;
    #[test] fn bounds_and_extremes() {
        assert_eq!(clamp(-1, 0, 10), 0);
        assert_eq!(clamp(11, 0, 10), 10);
        assert_eq!(clamp(5, 0, 10), 5);
        assert_eq!(clamp(0, 0, 10), 0);
        assert_eq!(clamp(10, 0, 10), 10);
        assert_eq!(clamp(i32::MIN, -2, 3), -2);
        assert_eq!(clamp(i32::MAX, -2, 3), 3);
        assert_eq!(clamp(7, 4, 4), 4);
    }
}
