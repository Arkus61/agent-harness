use fixture::parse_port;
#[test] fn accepted_decimal_ports() {
    for (text, want) in [("1",1), ("80",80), ("00080",80), ("65535",65535)] {
        assert_eq!(parse_port(text), Ok(want), "{text:?}");
    }
}
#[test] fn rejected_forms() {
    for text in ["", "0", "000", "65536", "999999999999999999999999", "-1", "+1", " 1", "1 ", "1\n", "1.0", "１", "١", "1_000"] {
        assert_eq!(parse_port(text), Err("invalid port"), "{text:?}");
    }
}
