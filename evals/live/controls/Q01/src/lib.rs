/// Parse an ASCII decimal TCP port in the inclusive range 1..=65535.
pub fn parse_port(input: &str) -> Result<u16, &'static str> {
    if input.is_empty() || !input.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid port");
    }
    match input.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err("invalid port"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_port;
    #[test] fn port_regressions() {
        assert_eq!(parse_port("00080"), Ok(80));
        assert_eq!(parse_port("65535"), Ok(65535));
        for bad in ["", "0", "65536", "+1", " 1", "1 ", "-1", "１"] {
            assert_eq!(parse_port(bad), Err("invalid port"));
        }
    }
}
