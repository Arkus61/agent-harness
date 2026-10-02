/// Parse a line and return quantity * unit_price_cents without u32 overflow.
pub fn invoice_total(line: &str) -> Option<u64> {
    let (quantity, price) = crate::parser::parse_line(line)?;
    Some(u64::from(quantity) * u64::from(price))
}
#[cfg(test)] mod tests {
    use super::invoice_total;
    #[test] fn integrated_regressions() {
        assert_eq!(invoice_total("2,105"), Some(210));
        assert_eq!(invoice_total("0,20"), Some(0));
        assert_eq!(invoice_total("4294967295,4294967295"), Some(18446744065119617025));
        assert_eq!(invoice_total("2,3,4"), None);
    }
}
