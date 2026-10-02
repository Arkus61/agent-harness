use fixture::{parse_line,invoice_total};
#[test] fn parser_order_and_whitespace() {
    for (text,want) in [("2,105",(2,105)),(" 2 , 105 ",(2,105)),("\t0002,00105\r\n",(2,105)),("0,0",(0,0)),("4294967295,4294967295",(u32::MAX,u32::MAX))] {
        assert_eq!(parse_line(text),Some(want),"{text:?}");
    }
}
#[test] fn malformed_lines_rejected() {
    for text in ["", "2", "2,3,4", "2,3,", ",3", "2,", "+2,3", "2,-3", "2,4294967296", "２,3", "2,٣", "\u{a0}2,3", "2,3\u{a0}"] {
        assert_eq!(parse_line(text),None,"{text:?}");
        assert_eq!(invoice_total(text),None,"{text:?}");
    }
}
#[test] fn integration_multiplication_and_overflow() {
    for (text,want) in [("2,105",210),("0,105",0),("3,0",0),("4294967295,4294967295",18446744065119617025)] {
        assert_eq!(invoice_total(text),Some(want));
    }
}
