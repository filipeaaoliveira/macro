use std::collections::HashSet;

use macro_user_id::user_id::MacroUserIdStr;

use super::names;

/// The authentication service answers with the lowercased ids it stores,
/// and a caller may name an account with capitals in its email.
#[test]
fn an_account_is_found_whatever_the_case_it_was_named_in() {
    let existing = HashSet::from(["macro|julia@example.com".to_string()]);
    let named = |id: &str| MacroUserIdStr::try_from(id.to_string()).expect("valid");

    assert!(names(&existing, &named("macro|Julia@Example.com")));
    assert!(names(&existing, &named("macro|julia@example.com")));
    assert!(!names(&existing, &named("macro|teo@example.com")));
}
