/// Decimal units with exactly two digits after the point.
pub fn format_cents(cents: u64) -> String {
    format!("{}.{}", cents / 100, cents % 100)
}
