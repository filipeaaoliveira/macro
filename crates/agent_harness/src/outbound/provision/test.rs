use super::{CHECKOUT, ENSURE_READY_SCRIPT, SIDECAR_PORT};
use crate::domain::model::{EGRESS_URL_VARIABLE, SESSION_TOKEN_VARIABLE};

/// The script and the Rust constants name the same things. Asserted rather
/// than commented because the two are edited months apart, and a rename on
/// either side would otherwise show up only as a sandbox that never clones.
#[test]
fn the_script_and_the_harness_agree_on_shared_names() {
    assert!(ENSURE_READY_SCRIPT.contains(&format!("sidecar_port={SIDECAR_PORT}")));
    assert!(ENSURE_READY_SCRIPT.contains(&format!("${{{EGRESS_URL_VARIABLE}%/}}/git")));
    assert!(ENSURE_READY_SCRIPT.contains(&format!("${SESSION_TOKEN_VARIABLE}")));
    assert!(ENSURE_READY_SCRIPT.contains(&format!("workspace_dir={CHECKOUT}\n")));
}

/// A command runs from the checkout under its own timeout, as one `bash -c`
/// line a provider exec API runs without a shell of its own.
#[test]
fn a_session_command_runs_in_the_checkout() {
    let command = agent_session::domain::session_command::SessionCommand {
        cwd: ".".into(),
        command: "just check".into(),
        timeout_secs: 60,
    };

    let line = super::session_command(&command);

    assert!(line.starts_with(r"bash -c 'cd '\''/workspace'\'' || exit 1; "));
    assert!(line.contains("timeout --kill-after=10 60"));
    assert!(line.ends_with(r"bash --noprofile --norc -c '\''just check'\'''"));
}

/// The sandbox holds no GitHub credential and is told no repository: it clones
/// from the proxy, which reads both off the session's own grant.
#[test]
fn clone_uses_session_egress_without_a_github_token() {
    assert!(!ENSURE_READY_SCRIPT.contains("GITHUB_TOKEN"));
    assert!(!ENSURE_READY_SCRIPT.contains("REPO_URL"));
    assert!(!ENSURE_READY_SCRIPT.contains("gh auth"));
}
