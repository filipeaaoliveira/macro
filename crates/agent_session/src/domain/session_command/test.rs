use super::*;

fn command(cwd: &str, line: &str, timeout_secs: u64) -> SessionCommand {
    SessionCommand {
        cwd: cwd.to_owned(),
        command: line.to_owned(),
        timeout_secs,
    }
}

#[test]
fn a_command_must_stay_inside_the_checkout() {
    assert_eq!(command(".", "just check", 60).validate(), Ok(()));
    assert_eq!(command("apps/web", "bun test", 60).validate(), Ok(()));
    assert_eq!(
        command("/etc", "cat passwd", 60).validate(),
        Err(InvalidSessionCommand::OutsideCheckout)
    );
    assert_eq!(
        command("apps/../..", "ls", 60).validate(),
        Err(InvalidSessionCommand::OutsideCheckout)
    );
}

#[test]
fn a_command_needs_a_line_and_a_bounded_timeout() {
    assert_eq!(
        command(".", "  ", 60).validate(),
        Err(InvalidSessionCommand::EmptyCommand)
    );
    assert_eq!(
        command(".", "ls", 0).validate(),
        Err(InvalidSessionCommand::Timeout)
    );
    assert_eq!(
        command(".", "ls", 30 * 60 + 1).validate(),
        Err(InvalidSessionCommand::Timeout)
    );
}

#[test]
fn the_script_runs_the_quoted_command_in_its_directory_under_its_timeout() {
    assert_eq!(
        command("apps/web", "echo 'it''s' && exit 3", 90).script("/workspace"),
        r"cd '/workspace/apps/web' || exit 1; exec timeout --kill-after=10 90 bash --noprofile --norc -c 'echo '\''it'\'''\''s'\'' && exit 3'"
    );
    assert!(
        command(".", "ls", 5)
            .script("/workspace/")
            .starts_with("cd '/workspace/' || exit 1; ")
    );
}

/// Run by bash for real: a directory that is not there runs nothing.
#[test]
fn nothing_runs_where_the_directory_is_missing() {
    let checkout = std::env::temp_dir().join(format!("session-command-{}", std::process::id()));
    std::fs::create_dir_all(&checkout).unwrap();
    let marker = checkout.join("ran");
    let script = command("missing", &format!("touch '{}'", marker.display()), 5)
        .script(checkout.to_str().unwrap());

    let status = std::process::Command::new("bash")
        .args(["-c", &script])
        .status()
        .unwrap();
    let ran = marker.exists();
    std::fs::remove_dir_all(&checkout).unwrap();

    assert_eq!(status.code(), Some(1));
    assert!(!ran);
}

/// A cut never splits a character: the kept end starts on a boundary.
#[test]
fn output_is_cut_on_a_character_boundary() {
    // Three-byte characters, so the cut falls inside one.
    let long = "€".repeat(MAX_COMMAND_OUTPUT_BYTES);
    let output = SessionCommandOutput::new(0, long.as_bytes(), b"");

    assert!(output.stdout.len() <= MAX_COMMAND_OUTPUT_BYTES);
    assert!(
        output
            .stdout
            .trim_start_matches("(earlier output left out)\n")
            .chars()
            .all(|c| c == '€')
    );
}

#[test]
fn output_keeps_the_end_of_each_stream() {
    let long = format!("{}{}", "a".repeat(MAX_COMMAND_OUTPUT_BYTES), "the end");
    let output = SessionCommandOutput::new(1, long.as_bytes(), b"why");

    assert_eq!(output.exit_code, 1);
    assert_eq!(output.stderr, "why");
    assert!(output.stdout.len() <= MAX_COMMAND_OUTPUT_BYTES);
    assert!(output.stdout.starts_with("(earlier output left out)\n"));
    assert!(output.stdout.ends_with("the end"));
}
