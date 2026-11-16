use std::collections::HashSet;

use super::names;

/// The authentication service stores ids lowercased, and a caller may name
/// an account with capitals in its email.
#[test]
fn an_account_is_found_whatever_the_case_of_its_id() {
    let existing = HashSet::from(["macro|julia@example.com".to_string()]);

    assert!(names(&existing, "macro|Julia@Example.com"));
    assert!(names(&existing, "macro|julia@example.com"));
    assert!(!names(&existing, "macro|teo@example.com"));
}
