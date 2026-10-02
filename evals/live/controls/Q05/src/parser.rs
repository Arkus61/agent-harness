/// Parse quantity,unit_price_cents: exactly two nonempty ASCII decimal u32 values.
/// Surrounding ASCII whitespace is accepted for each field; signs are rejected.
pub fn parse_line(line: &str) -> Option<(u32, u32)> {
    let fields: Vec<_> = line.split(',').collect();
    if fields.len() != 2 { return None; }
    fn decimal(s: &str) -> Option<u32> {
        let s = s.trim_matches(|c: char| c.is_ascii_whitespace());
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
        s.parse().ok()
    }
    Some((decimal(fields[0])?, decimal(fields[1])?))
}
#[cfg(test)] mod tests {
    use super::parse_line;
    #[test] fn parser_regressions() {
        assert_eq!(parse_line(" 2 , 105 "), Some((2, 105)));
        for bad in ["", "2", "2,3,4", ",3", "+2,3", "2,-3", "2,４", "2,4294967296"] {
            assert_eq!(parse_line(bad), None);
        }
    }
}
