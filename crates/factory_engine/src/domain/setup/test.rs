use super::{RepositorySetup, SetupFileError};

const SETUP: &str = r#"{
    "setup": [
        { "name": "install", "run": "bun install --frozen-lockfile" },
        { "name": "generate", "run": "just generate", "timeout": 300 }
    ],
    "env": { "CARGO_TARGET_DIR": "/tmp/target", "npm_config_cache": "/tmp/npm" }
}"#;

#[test]
fn a_setup_reads_its_commands_in_order_and_its_env() {
    let setup = RepositorySetup::parse(SETUP).expect("parse");

    let names: Vec<&str> = setup
        .setup
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert_eq!(names, ["install", "generate"]);
    assert_eq!(setup.setup[0].timeout, 600);
    assert_eq!(setup.setup[1].timeout, 300);
    assert_eq!(setup.env["CARGO_TARGET_DIR"], "/tmp/target");
    assert!(!setup.is_empty());
}

#[test]
fn an_empty_setup_asks_for_nothing() {
    let setup = RepositorySetup::parse("{}").expect("parse");

    assert!(setup.is_empty());
    assert_eq!(setup.exports(), "");
}

#[test]
fn a_command_runs_with_the_env_under_its_timeout() {
    let setup = RepositorySetup::parse(
        r#"{"setup":[{"name":"install","run":"echo 'hi' && make"}],"env":{"A":"it's"}}"#,
    )
    .expect("parse");

    assert_eq!(
        setup.shell(&setup.setup[0]),
        "export A='it'\\''s'\ntimeout --kill-after=10 600 bash --noprofile --norc -c 'echo '\\''hi'\\'' && make'"
    );
}

#[test]
fn a_file_that_breaks_the_rules_says_why() {
    let long_run = "x".repeat(4097);
    let many: Vec<String> = (0..17)
        .map(|at| format!(r#"{{"name":"c{at}","run":"true"}}"#))
        .collect();
    let many = format!(r#"{{"setup":[{}]}}"#, many.join(","));
    let many_env: Vec<String> = (0..33).map(|at| format!(r#""V{at}":"x""#)).collect();
    let many_env = format!(r#"{{"env":{{{}}}}}"#, many_env.join(","));
    let long_value = format!(r#"{{"env":{{"A":"{}"}}}}"#, "x".repeat(1025));
    let long_run = format!(r#"{{"setup":[{{"name":"a","run":"{long_run}"}}]}}"#);
    let cases: Vec<(&str, fn(&SetupFileError) -> bool)> = vec![
        ("{", |e| matches!(e, SetupFileError::Unreadable(_))),
        (r#"{"setup":[],"archive":[]}"#, |e| {
            matches!(e, SetupFileError::Unreadable(_))
        }),
        (r#"{"setup":[{"name":"a","run":"x","wait":false}]}"#, |e| {
            matches!(e, SetupFileError::Unreadable(_))
        }),
        (r#"{"env":{},"env":{}}"#, |e| {
            matches!(e, SetupFileError::Unreadable(_))
        }),
        (&many, |e| matches!(e, SetupFileError::TooMany(17))),
        (r#"{"setup":[{"name":"Install","run":"x"}]}"#, |e| {
            matches!(e, SetupFileError::Name(_))
        }),
        (r#"{"setup":[{"name":"","run":"x"}]}"#, |e| {
            matches!(e, SetupFileError::Name(_))
        }),
        (
            r#"{"setup":[{"name":"a","run":"x"},{"name":"a","run":"y"}]}"#,
            |e| matches!(e, SetupFileError::DuplicateName(_)),
        ),
        (r#"{"setup":[{"name":"a","run":"  "}]}"#, |e| {
            matches!(e, SetupFileError::Run(_))
        }),
        (&long_run, |e| matches!(e, SetupFileError::Run(_))),
        (r#"{"setup":[{"name":"a","run":"x","timeout":0}]}"#, |e| {
            matches!(e, SetupFileError::Timeout(_))
        }),
        (r#"{"setup":[{"name":"a","run":"x","timeout":1201}]}"#, |e| {
            matches!(e, SetupFileError::Timeout(_))
        }),
        (&many_env, |e| matches!(e, SetupFileError::TooManyEnv(33))),
        (r#"{"env":{"1A":"x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"A-B":"x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"BASH_ENV":"/tmp/x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"MACRO_VERIFY_COMMIT":"x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (&long_value, |e| matches!(e, SetupFileError::EnvValue(_))),
        (r#"{"env":{"A":"x\ny"}}"#, |e| {
            matches!(e, SetupFileError::EnvValue(_))
        }),
    ];
    for (text, expected) in cases {
        let error = RepositorySetup::parse(text).expect_err(text);
        assert!(expected(&error), "{text}: {error}");
    }
}

#[test]
fn a_file_over_its_size_does_not_read() {
    let text = format!(r#"{{"env":{{"A":"{}"}}}}"#, " ".repeat(16 * 1024));

    assert_eq!(
        RepositorySetup::parse(&text),
        Err(SetupFileError::TooLarge)
    );
}

#[test]
fn a_file_holding_a_secret_does_not_read() {
    let cases = [
        concat!(
            r#"{"env":{"REGISTRY":"ghp_"#,
            r#"abcdefghijklmnopqrstuvwxyz0123"}}"#
        ),
        concat!(
            r#"{"setup":[{"name":"login","run":"npm login --auth sk-ant-"#,
            r#"abcdefghijklmnopqrstu"}]}"#
        ),
        r#"{"env":{"URL":"https://user:hunter2@registry.example.com"}}"#,
        // An escape hides the token from the text, not once decoded.
        r#"{"env":{"REGISTRY":"ghp\u005fabcdefghijklmnopqrstuvwxyz0123"}}"#,
    ];
    for text in cases {
        assert!(
            matches!(
                RepositorySetup::parse(text),
                Err(SetupFileError::Secret(_))
            ),
            "{text}"
        );
    }
}

#[test]
fn an_env_name_that_says_it_holds_a_secret_does_not_read() {
    for name in ["NPM_TOKEN", "db_password", "MY_API_KEY", "AWS_SECRET"] {
        let text = format!(r#"{{"env":{{"{name}":"x"}}}}"#);
        assert_eq!(
            RepositorySetup::parse(&text),
            Err(SetupFileError::SecretEnv(name.to_string())),
            "{name}"
        );
    }
}
