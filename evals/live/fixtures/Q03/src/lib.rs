/// Return a shipping rate in whole currency units for known regions.
pub fn shipping_rate(region: &str, expedited: bool) -> Option<u32> {
    if expedited {
        if region == "local" { Some(9) }
        else if region == "remote" { Some(20) }
        else { None }
    } else if region == "local" { Some(5) }
    else if region == "remote" { Some(12) }
    else { None }
}
