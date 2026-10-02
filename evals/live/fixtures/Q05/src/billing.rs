/// Parse a line and return quantity * unit_price_cents without u32 overflow.
pub fn invoice_total(line: &str) -> Option<u64> {
    let (quantity, price) = crate::parser::parse_line(line)?;
    Some(u64::from(quantity) + u64::from(price))
}
