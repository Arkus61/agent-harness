use fixture::{add_percent,format_cents};
#[test] fn arithmetic_floor_and_wide_values() {
    for (value, percent) in [(0,0),(0,u32::MAX),(1,1),(3,50),(200,10),(u32::MAX,0),(u32::MAX,u32::MAX)] {
        let want=(u128::from(value)*(100+u128::from(percent))/100) as u64;
        assert_eq!(add_percent(value,percent),want,"{value}, {percent}");
    }
}
#[test] fn decimal_formatting() {
    for (cents,want) in [(0,"0.00"),(1,"0.01"),(99,"0.99"),(100,"1.00"),(105,"1.05"),(999,"9.99"),(u64::MAX,"184467440737095516.15")] {
        assert_eq!(format_cents(cents),want);
    }
}
#[test] fn modules_compose() {
    assert_eq!(format_cents(add_percent(105,10)),"1.15");
}
