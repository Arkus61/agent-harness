/// Decimal units with exactly two digits after the point.
pub fn format_cents(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}
#[cfg(test)] mod tests {
    use super::format_cents;
    #[test] fn formatting_regressions() {
        assert_eq!(format_cents(0), "0.00");
        assert_eq!(format_cents(105), "1.05");
        assert_eq!(format_cents(999), "9.99");
        assert_eq!(format_cents(u64::MAX), "184467440737095516.15");
    }
}
