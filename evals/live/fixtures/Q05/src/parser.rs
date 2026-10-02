/// Parse quantity,unit_price_cents: exactly two nonempty ASCII decimal u32 values.
/// Surrounding ASCII whitespace is accepted for each field; signs are rejected.
pub fn parse_line(line: &str) -> Option<(u32, u32)> {
    let mut parts = line.split(',');
    let quantity = parts.next()?.trim().parse().ok()?;
    let price = parts.next()?.trim().parse().ok()?;
    Some((price, quantity))
}
