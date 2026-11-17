use std::sync::Mutex;

use agent_client_protocol::schema::v1::SessionId;
use agent_harness::domain::model::{ListedSessions, RuntimeAccessFacts};
use macro_user_id::user_id::MacroUserIdStr;
use tokio::io::AsyncWriteExt as _;

use super::*;

/// Binds every bot to one harness, or to none.
struct Bound(Option<HarnessId>);

impl HarnessBindings for Bound {
    async fn harness_for(&self, _bot: BotId) -> anyhow::Result<Option<HarnessId>> {
        Ok(self.0)
    }

    async fn access_facts(
        &self,
        _bot: BotId,
        _actor: Option<&MacroUserIdStr<'static>>,
    ) -> anyhow::Result<Option<RuntimeAccessFacts>> {
        unimplemented!("commands never ask for access")
    }

    async fn sessions_on_harness(
        &self,
        _harness: HarnessId,
        _acp_sessions: &[SessionId],
    ) -> anyhow::Result<ListedSessions> {
        unimplemented!("commands never list sessions")
    }
}

/// One computer, whose guest has a checkout or not, and whose command
/// writes `output` and exits with `exit_code`. Records each program run.
struct Guest {
    computer: Option<ComputerId>,
    checkout: bool,
    output: Vec<u8>,
    exit_code: i32,
    ran: Mutex<Vec<Vec<String>>>,
}

impl Guest {
    fn new(computer: Option<ComputerId>, checkout: bool, output: Vec<u8>, exit_code: i32) -> Self {
        Self {
            computer,
            checkout,
            output,
            exit_code,
            ran: Mutex::new(Vec::new()),
        }
    }
}

impl Guests for Guest {
    fn computer_of(
        &self,
        _harness: HarnessId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ComputerId>>> + Send + '_>> {
        Box::pin(async move { Ok(self.computer) })
    }

    fn exec<'a>(
        &'a self,
        _computer: ComputerId,
        argv: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<GuestProcess<String>>> + Send + 'a>> {
        self.ran
            .lock()
            .unwrap()
            .push(argv.iter().map(|arg| (*arg).to_owned()).collect());
        let (output, exit_code) = if argv[0] == "test" {
            (Vec::new(), if self.checkout { 0 } else { 1 })
        } else {
            (self.output.clone(), self.exit_code)
        };
        Box::pin(async move { Ok(process(output, exit_code)) })
    }
}

/// A program that writes `output` and exits with `exit_code`.
fn process(output: Vec<u8>, exit_code: i32) -> GuestProcess<String> {
    let (stdin, _program_stdin) = tokio::io::duplex(1024);
    let (mut program_stdout, stdout) = tokio::io::duplex(64 * 1024);
    let program = tokio::spawn(async move {
        program_stdout.write_all(&output).await.unwrap();
    });
    GuestProcess {
        stdin: Box::pin(stdin),
        stdout: Box::pin(stdout),
        exit: Box::pin(async move {
            program.await.unwrap();
            Ok(exit_code)
        }),
    }
}

fn harness() -> HarnessId {
    HarnessId::new_from_uuid(macro_uuid::Uuid::from_u128(1))
}

fn computer() -> ComputerId {
    ComputerId::new_from_uuid(macro_uuid::Uuid::from_u128(2))
}

fn session() -> AgentSessionId {
    AgentSessionId::new_from_uuid(macro_uuid::Uuid::from_u128(3))
}

fn check() -> SessionCommand {
    SessionCommand {
        cwd: ".".into(),
        command: "just check".into(),
        timeout_secs: 60,
    }
}

#[tokio::test]
async fn a_command_runs_in_the_sessions_worktree_on_its_computer() {
    let commands = ComputerCommands::new(
        Bound(Some(harness())),
        Guest::new(Some(computer()), true, b"error: 1 failed".to_vec(), 101),
    );

    let output = commands
        .exec(BotId::TEST_A, session(), check())
        .await
        .unwrap();

    assert_eq!(
        output,
        SessionCommandOutput::new(101, b"error: 1 failed", b"")
    );
    let checkout = format!(
        "/home/macro/.macrod/herdr/{}/worktrees/{}",
        harness(),
        session()
    );
    assert_eq!(
        *commands.guests.ran.lock().unwrap(),
        [
            vec!["test".to_owned(), "-d".to_owned(), checkout.clone()],
            vec![
                "bash".to_owned(),
                "-c".to_owned(),
                format!("{{ {}; }} 2>&1", check().script(&checkout)),
            ],
        ]
    );
}

#[tokio::test]
async fn only_the_end_of_a_long_output_is_kept() {
    let mut long = vec![b'a'; 3 * MAX_COMMAND_OUTPUT_BYTES];
    long.extend_from_slice(b"the end");
    let commands = ComputerCommands::new(
        Bound(Some(harness())),
        Guest::new(Some(computer()), true, long, 1),
    );

    let output = commands
        .exec(BotId::TEST_A, session(), check())
        .await
        .unwrap();

    assert!(output.stdout.len() <= MAX_COMMAND_OUTPUT_BYTES);
    assert!(output.stdout.ends_with("the end"));
}

/// A session on a computer whose checkout is not where macrod keeps it
/// fails, so a retry looks again: it never falls back to the agent's
/// report.
#[tokio::test]
async fn a_session_on_a_computer_without_its_checkout_fails() {
    let commands = ComputerCommands::new(
        Bound(Some(harness())),
        Guest::new(Some(computer()), false, Vec::new(), 0),
    );

    let error = commands
        .exec(BotId::TEST_A, session(), check())
        .await
        .unwrap_err();

    assert!(matches!(error, HarnessError::Container(_)), "{error:?}");
    assert_eq!(commands.guests.ran.lock().unwrap().len(), 1);
}

/// A paired machine of someone's own, and an unbound bot, run nothing, and
/// say so.
#[tokio::test]
async fn a_session_on_no_computer_runs_nothing() {
    for (bound, guest) in [
        (
            Bound(None),
            Guest::new(Some(computer()), true, Vec::new(), 0),
        ),
        (
            Bound(Some(harness())),
            Guest::new(None, true, Vec::new(), 0),
        ),
    ] {
        let commands = ComputerCommands::new(bound, guest);

        let refused = commands
            .exec(BotId::TEST_A, session(), check())
            .await
            .unwrap_err();

        assert!(
            matches!(
                refused,
                HarnessError::Session(AgentSessionError::CommandUnsupported(id)) if id == session()
            ),
            "{refused:?}"
        );
        assert!(commands.guests.ran.lock().unwrap().is_empty());
    }
}
