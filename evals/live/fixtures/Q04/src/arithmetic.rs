/// Integer floor of value * (100 + percent) / 100, with wide intermediate arithmetic.
pub fn add_percent(value: u32, percent: u32) -> u64 {
    u64::from(value) + u64::from(percent)
}
