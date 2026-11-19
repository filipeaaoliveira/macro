use agent_session::domain::session_command::SessionCommandOutput;

use super::{REDACTED, RepositorySetup, SetupFileError, output_tail, redacted};
use crate::domain::shell_lock::{LOCK_BUSY, LOCK_BUSY_STATUS};

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

/// The values are exported inside the script `bash` runs, never before
/// `timeout` or `bash`, and single-quoted, so they are never expanded.
#[test]
fn a_command_runs_with_the_env_under_its_timeout() {
    let setup = RepositorySetup::parse(
        r#"{"setup":[{"name":"install","run":"make"}],"env":{"PATH":"$PATH:/opt/bin"}}"#,
    )
    .expect("parse");

    assert_eq!(
        setup.shell(&setup.setup[0]),
        "timeout --kill-after=10 600 bash --noprofile --norc -c 'export PATH='\\''$PATH:/opt/bin'\\''\nmake'"
    );
}

/// Whether an error is the one a case expects.
type Expected = fn(&SetupFileError) -> bool;

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
    let cases: Vec<(&str, Expected)> = vec![
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
        (r#"{"env":{"LD_PRELOAD":"/tmp/x.so"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"DYLD_INSERT_LIBRARIES":"x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"GIT_CONFIG_GLOBAL":"/tmp/x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"GIT_DIR":"/tmp/x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"SHELLOPTS":"xtrace"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"PS4":"x"}}"#, |e| {
            matches!(e, SetupFileError::EnvName(_))
        }),
        (r#"{"env":{"HOME":"/tmp"}}"#, |e| {
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
    for name in [
        "NPM_TOKEN",
        "db_password",
        "MY_API_KEY",
        "AWS_SECRET",
        "SECRET_ACCESS_KEY",
    ] {
        let text = format!(r#"{{"env":{{"{name}":"x"}}}}"#);
        assert_eq!(
            RepositorySetup::parse(&text),
            Err(SetupFileError::SecretEnv(name.to_string())),
            "{name}"
        );
    }
}

/// A word of a name only holds a secret whole: these are plain values.
#[test]
fn an_env_name_holding_a_secret_word_only_in_part_reads() {
    let setup = RepositorySetup::parse(
        r#"{"env":{"TOKENIZERS_PARALLELISM":"false","GOOGLE_APPLICATION_CREDENTIALS":"/etc/sa.json","KEYBOARD":"us"}}"#,
    )
    .expect("parse");

    assert_eq!(setup.env.len(), 3);
}

/// What a failed setup shows of its output: both streams, each line that
/// looks like a secret left out, no control character but line breaks and
/// tabs, and only its end.
#[test]
fn a_setup_s_output_tail_leaves_secrets_and_control_characters_out() {
    let output = SessionCommandOutput {
        exit_code: 1,
        stdout: format!("{}\nfetching\u{1b}[0m\ttwo", "x".repeat(10_000)),
        stderr: concat!("token: ghp_", "abcdefghijklmnopqrstuvwxyz0123\nerror: 401").to_string(),
    };

    let tail = output_tail(&output);

    assert!(tail.ends_with(&format!("fetching[0m\ttwo\n{REDACTED}\nerror: 401")), "{tail}");
    assert!(!tail.contains("ghp_"));
    assert!(tail.len() <= 4096);
    assert!(tail.starts_with('x'));
}

/// Run by bash in a git checkout of its own, a session's setup command
/// that finds the checkout's setup lock held by a live run (here the
/// `mkdir` lock, which every machine has) gives up, saying so, without
/// running.
#[test]
fn a_session_setup_command_waits_for_the_checkout_s_lock() {
    use std::process::Command;

    let root = std::env::temp_dir().join(format!("macro-setup-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "--quiet"]).status.success());
    let lock = String::from_utf8(git(&["rev-parse", "--git-path", "macro-setup.lock"]).stdout)
        .unwrap()
        .trim()
        .to_owned();
    let holder = Command::new("sleep").arg("30").spawn().unwrap();
    let held = root.join(format!("{lock}.d"));
    std::fs::create_dir_all(&held).unwrap();
    std::fs::write(held.join("pid"), holder.id().to_string()).unwrap();
    let setup = RepositorySetup::parse(r#"{ "setup": [{ "name": "a", "run": "echo ran" }] }"#)
        .expect("parse");
    // Without `flock` on its `PATH`, the run takes the `mkdir` lock.
    let tools = root.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    for tool in ["git", "mkdir", "cat", "mv", "rm", "sleep", "bash", "timeout"] {
        let found = Command::new("bash")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        let found = String::from_utf8(found.stdout).unwrap();
        if !found.trim().is_empty() {
            let _ = std::os::unix::fs::symlink(found.trim(), tools.join(tool));
        }
    }

    let busy = Command::new("/bin/bash")
        .args(["--noprofile", "--norc", "-c", &setup.locked_shell(&setup.setup[0], 1)])
        .current_dir(&root)
        .env("PATH", &tools)
        .output()
        .unwrap();
    let mut holder = holder;
    let _ = holder.kill();
    let _ = holder.wait();
    let _ = std::fs::remove_dir_all(&root);

    let stderr = String::from_utf8_lossy(&busy.stderr);
    assert_eq!(busy.status.code(), Some(LOCK_BUSY_STATUS), "{stderr}");
    assert!(stderr.contains(LOCK_BUSY), "{stderr}");
    assert!(!String::from_utf8_lossy(&busy.stdout).contains("ran"));
}

/// A private key's block is left out whole, its body too, whose lines
/// match no marker of their own; what follows its end is kept, and a block
/// that never ends is left out to the end of the output.
#[test]
fn a_private_key_s_whole_block_is_left_out() {
    let body = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7";
    let text = format!(
        "generating\n-----BEGIN OPENSSH PRIVATE KEY-----\n{body}\n{body}\n-----END OPENSSH PRIVATE KEY-----\ndone\n-----BEGIN RSA PRIVATE KEY-----\n{body}"
    );

    let kept = redacted(&text);

    assert!(!kept.contains(body), "{kept}");
    let lines: Vec<&str> = kept.split('\n').collect();
    assert_eq!(lines[0], "generating");
    assert!(lines[1..5].iter().all(|line| *line == REDACTED), "{kept}");
    assert_eq!(lines[5], "done");
    assert!(lines[6..].iter().all(|line| *line == REDACTED), "{kept}");
}

/// A certificate's block is public, and kept.
#[test]
fn a_certificate_s_block_is_kept() {
    let text = "-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIU\n-----END CERTIFICATE-----";

    assert_eq!(redacted(text), text);
}
