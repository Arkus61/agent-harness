use fixture::clamp;
#[test] fn all_relative_positions() {
    for (value,low,high,want) in [(-3,0,10,0), (0,0,10,0), (3,0,10,3), (10,0,10,10), (13,0,10,10), (-11,-10,-2,-10), (-4,-10,-2,-4), (0,-10,-2,-2)] {
        assert_eq!(clamp(value,low,high), want);
    }
}
#[test] fn singleton_and_extreme_ranges() {
    for value in [i32::MIN,-1,0,1,i32::MAX] {
        assert_eq!(clamp(value,4,4),4);
        assert_eq!(clamp(value,i32::MIN,i32::MAX),value);
    }
    assert_eq!(clamp(i32::MIN,-2,3),-2);
    assert_eq!(clamp(i32::MAX,-2,3),3);
}
