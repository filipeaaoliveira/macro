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
        r"cd '/workspace/apps/web' && exec timeout --kill-after=10 90 bash -lc 'echo '\''it'\'''\''s'\'' && exit 3'"
    );
    assert_eq!(
        command(".", "ls", 5).script("/workspace/"),
        "cd '/workspace/' && exec timeout --kill-after=10 5 bash -lc 'ls'"
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
