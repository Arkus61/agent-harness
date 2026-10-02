/// Single table of (standard, expedited) shipping prices.
fn rate_table(region: &str) -> Option<(u32, u32)> {
    match region {
        "local" => Some((5, 9)),
        "remote" => Some((12, 20)),
        _ => None,
    }
}

/// Return a shipping rate in whole currency units for known regions.
pub fn shipping_rate(region: &str, expedited: bool) -> Option<u32> {
    rate_table(region).map(|(standard, fast)| if expedited { fast } else { standard })
}

#[cfg(test)]
mod tests {
    use super::shipping_rate;
    #[test] fn all_shipping_rates() {
        assert_eq!(shipping_rate("local", false), Some(5));
        assert_eq!(shipping_rate("local", true), Some(9));
        assert_eq!(shipping_rate("remote", false), Some(12));
        assert_eq!(shipping_rate("remote", true), Some(20));
        assert_eq!(shipping_rate("unknown", false), None);
        assert_eq!(shipping_rate("unknown", true), None);
    }
}
