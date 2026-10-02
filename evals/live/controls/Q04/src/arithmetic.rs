/// Integer floor of value * (100 + percent) / 100, with wide intermediate arithmetic.
pub fn add_percent(value: u32, percent: u32) -> u64 {
    (u128::from(value) * (100 + u128::from(percent)) / 100) as u64
}
#[cfg(test)] mod tests {
    use super::add_percent;
    #[test] fn arithmetic_regressions() {
        assert_eq!(add_percent(200, 10), 220);
        assert_eq!(add_percent(3, 50), 4);
        assert_eq!(add_percent(0, 10), 0);
        assert_eq!(add_percent(20, 0), 20);
        assert_eq!(add_percent(u32::MAX, u32::MAX), 184467444946163465);
    }
}
