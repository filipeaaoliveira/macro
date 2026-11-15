use super::{EntryId, Outcome};

#[test]
fn a_flow_name_is_lowercase_letters_digits_dashes_and_underscores() {
    assert!(EntryId::try_from("plan-review_2".to_string()).is_ok());
    assert!(Outcome::try_from("tests_only".to_string()).is_ok());

    assert!(EntryId::try_from(String::new()).is_err());
    assert!(EntryId::try_from("Plan".to_string()).is_err());
    assert!(EntryId::try_from("end:success".to_string()).is_err());
    assert!(Outcome::try_from("a".repeat(65)).is_err());
}

#[test]
fn a_name_read_from_json_is_checked() {
    let refused = serde_json::from_str::<EntryId>(r#""two words""#);

    assert!(refused.is_err());
}
