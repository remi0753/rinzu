use super::*;

#[test]
fn private_failure_packet_preserves_order_and_unknowns_are_errors() {
    let packet = parse_packet("5@2:3:7,12@3:8:9,14@-,15@2:10:11,16@2:12:13")
        .unwrap()
        .unwrap();
    assert_eq!(packet["primary"]["cause"], "Error(Conversion)");
    assert_eq!(packet["primary"]["source"]["file"], 2);
    assert_eq!(packet["cleanup"][0]["cause"], "Error(Io)");
    assert_eq!(packet["cleanup"][1]["cause"], "Error(UnboundValue)");
    assert!(packet["cleanup"][1]["source"].is_null());
    assert_eq!(packet["cleanup"][2]["cause"], "Error(User)");
    assert_eq!(packet["cleanup"][2]["source"]["startByte"], 10);
    assert_eq!(packet["cleanup"][3]["cause"], "Panic");
    assert!(parse_packet("0").unwrap().is_none());
    for invalid in ["", "100@-", "5", "5@2:9:8", "5@2:0:4294967296", "5@oops"] {
        assert!(parse_packet(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn private_packet_is_bounded_without_reordering_the_primary() {
    let packet = std::iter::repeat_n("5@-", ROW_LIMIT + 3)
        .collect::<Vec<_>>()
        .join(",");
    let packet = parse_packet(&packet).unwrap().unwrap();
    assert_eq!(packet["cleanup"].as_array().unwrap().len(), ROW_LIMIT - 1);
    assert_eq!(packet["omittedCleanup"], 3);
}
