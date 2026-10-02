use harness_fixture::median3;

#[test]
fn permutations() {
    for values in [[1,2,3], [1,3,2], [2,1,3], [2,3,1], [3,1,2], [3,2,1]] {
        assert_eq!(median3(values[0], values[1], values[2]), 2);
    }
}
#[test]
fn repeats_negatives_extremes() {
    for (a,b,c,want) in [(1,1,2,1), (1,2,2,2), (-7,-9,-8,-8), (0,0,0,0),
        (i32::MIN,0,i32::MAX,0), (i32::MAX,i32::MIN,i32::MIN,i32::MIN)] {
        assert_eq!(median3(a,b,c), want);
    }
}
