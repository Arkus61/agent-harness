/// Parse an ASCII decimal TCP port in the inclusive range 1..=65535.
pub fn parse_port(_input: &str) -> Result<u16, &'static str> {
    Err("invalid port")
}
