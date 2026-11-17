use super::Config;

/// The settings every deployment gives, with `extra` beside them, as
/// `APP_SECRETS_JSON` gives them.
fn secrets(extra: &str) -> String {
    format!(
        r#"{{"DATABASE_URL":"postgres://localhost/macrodb","INTERNAL_API_KEY":"key",
        "TEMPORAL_ADDRESS":"http://127.0.0.1:7233","DOCUMENT_STORAGE_SERVICE_AUTH_KEY":"dss",
        "AUTHENTICATION_SERVICE_SECRET_KEY":"auth"{extra}}}"#
    )
}

/// `FACTORY_REVIEW_ROUNDS` bounds the review panel's rounds; unset leaves
/// the Flow's own bound, and zero rounds is refused. `FACTORY_QUESTIONS`
/// bounds each agent Step's questions, zero included; unset leaves the
/// engine's default.
#[test]
fn the_review_rounds_and_questions_are_read() {
    let read = |extra: &str| {
        // SAFETY: this is the only test in the binary that sets the
        // environment, and it reads it back before another can.
        unsafe { std::env::set_var("APP_SECRETS_JSON", secrets(extra)) };
        macro_config::ConfigLoader::load::<Config>()
    };

    let bounded = read(r#","FACTORY_REVIEW_ROUNDS":"5","FACTORY_QUESTIONS":"0""#).expect("reads");
    let unset = read("").expect("reads");
    let zero = read(r#","FACTORY_REVIEW_ROUNDS":"0""#);

    assert_eq!(
        bounded.factory_review_rounds.map(|rounds| rounds.get()),
        Some(5)
    );
    assert_eq!(unset.factory_review_rounds, None);
    assert_eq!(bounded.factory_questions, Some(0));
    assert_eq!(unset.factory_questions, None);
    assert!(zero.is_err());
}
