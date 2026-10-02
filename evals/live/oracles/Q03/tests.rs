use fixture::shipping_rate;
#[test] fn known_region_rates() {
    for (region, expedited, want) in [("local",false,Some(5)), ("local",true,Some(9)), ("remote",false,Some(12)), ("remote",true,Some(20))] {
        assert_eq!(shipping_rate(region,expedited),want);
    }
}
#[test] fn unknown_region_behavior_preserved() {
    for region in ["", "LOCAL", "Local", " remote ", "unknown", "ローカル"] {
        assert_eq!(shipping_rate(region,false),None);
        assert_eq!(shipping_rate(region,true),None);
    }
}
