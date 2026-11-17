//! Commands run in a session's checkout on the Macro computer its agent
//! works on, through the fleet's exec: the router's [`MachineCommands`] for
//! external sessions.
//!
//! A computer's macrod checks each session out in a Herdr worktree of its
//! own, under the guest user's `~/.macrod/herdr/<harness>/worktrees/<session>`.
//! A computer whose macrod keeps no such worktree (one configured to run
//! sessions in a shared workspace, or to store worktrees elsewhere) has no
//! checkout this finds, and runs nothing, as a paired machine of someone's
//! own does.

#[cfg(test)]
mod test;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use agent_harness::domain::error::{HarnessError, Result};
use agent_harness::domain::ports::{HarnessBindings, MachineCommands};
use agent_session::domain::error::AgentSessionError;
use agent_session::domain::model::AgentSessionId;
use agent_session::domain::session_command::{
    MAX_COMMAND_OUTPUT_BYTES, SessionCommand, SessionCommandOutput,
};
use bot_id::BotId;
use harness_id::HarnessId;
use hosted_runtimes::domain::ids::ComputerId;
use hosted_runtimes::domain::ports::{GUEST_USER, GuestProcess};
use tokio::io::AsyncReadExt as _;

use super::Operator;

/// How long finding the checkout may take.
const FIND_CHECKOUT_TIMEOUT: Duration = Duration::from_secs(30);

/// How much longer than its own timeout a command is given to reach the
/// guest and hand its output back.
const EXEC_MARGIN: Duration = Duration::from_secs(30);

/// The computers' guests, as running a command needs them.
pub(crate) trait Guests: Send + Sync + 'static {
    /// The computer `harness` runs on; none when it is not a computer's.
    fn computer_of(
        &self,
        harness: HarnessId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ComputerId>>> + Send + '_>>;

    /// Start `argv` in the computer's guest as its user.
    fn exec<'a>(
        &'a self,
        computer: ComputerId,
        argv: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<GuestProcess<String>>> + Send + 'a>>;
}

/// [`Guests`] through the operator and its fleet.
pub(crate) struct OperatorGuests(pub(crate) Arc<Operator>);

impl Guests for OperatorGuests {
    fn computer_of(
        &self,
        harness: HarnessId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ComputerId>>> + Send + '_>> {
        Box::pin(async move {
            self.0
                .computer_of(harness)
                .await
                .map_err(|error| HarnessError::Container(error.to_string()))
        })
    }

    fn exec<'a>(
        &'a self,
        computer: ComputerId,
        argv: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<GuestProcess<String>>> + Send + 'a>> {
        Box::pin(async move {
            use hosted_runtimes::domain::ports::Fleet as _;

            self.0
                .fleet()
                .exec(computer, argv)
                .await
                .map(|process| process.map_err(|error| error.to_string()))
                .map_err(|error| HarnessError::Container(error.to_string()))
        })
    }
}

/// Runs external sessions' commands on the Macro computers their bots'
/// harnesses run on.
pub(crate) struct ComputerCommands<Bindings, Machines> {
    bindings: Bindings,
    guests: Machines,
}

impl<Bindings, Machines> ComputerCommands<Bindings, Machines> {
    /// Find each bot's harness through `bindings`, and its computer's guest
    /// through `guests`.
    pub(crate) fn new(bindings: Bindings, guests: Machines) -> Self {
        Self { bindings, guests }
    }
}

impl<Bindings: HarnessBindings, Machines: Guests> MachineCommands
    for ComputerCommands<Bindings, Machines>
{
    fn exec(
        &self,
        bot: BotId,
        session: AgentSessionId,
        command: SessionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<SessionCommandOutput>> + Send + '_>> {
        Box::pin(async move {
            let unsupported =
                || HarnessError::Session(AgentSessionError::CommandUnsupported(session));
            let harness = self
                .bindings
                .harness_for(bot)
                .await
                .map_err(|error| HarnessError::Container(error.to_string()))?
                .ok_or_else(unsupported)?;
            let computer = self
                .guests
                .computer_of(harness)
                .await?
                .ok_or_else(unsupported)?;

            let checkout = checkout(harness, session);
            let found = self
                .run(computer, &["test", "-d", &checkout], FIND_CHECKOUT_TIMEOUT)
                .await?;
            if found.exit_code != 0 {
                return Err(unsupported());
            }

            // The guest streams stdout alone, so stderr joins it.
            let script = format!("{} 2>&1", command.script(&checkout));
            self.run(
                computer,
                &["bash", "-c", &script],
                command.timeout() + EXEC_MARGIN,
            )
            .await
        })
    }
}

impl<Bindings, Machines: Guests> ComputerCommands<Bindings, Machines> {
    /// Run `argv` in the guest, keeping the end of its output, within
    /// `timeout`. A program still running then is dropped, which stops it.
    async fn run(
        &self,
        computer: ComputerId,
        argv: &[&str],
        timeout: Duration,
    ) -> Result<SessionCommandOutput> {
        let ran = async {
            let process = self.guests.exec(computer, argv).await?;
            drop(process.stdin);
            let output = read_tail(process.stdout).await?;
            let exit_code = process
                .exit
                .await
                .map_err(|error| HarnessError::Container(error))?;
            Ok(SessionCommandOutput::new(exit_code, &output, b""))
        };
        tokio::time::timeout(timeout, ran).await.map_err(|_| {
            HarnessError::Container(format!(
                "computer {computer} did not finish the command in time"
            ))
        })?
    }
}

/// Where macrod checks `session` out for `harness` on a computer.
fn checkout(harness: HarnessId, session: AgentSessionId) -> String {
    format!(
        "{}/.macrod/herdr/{harness}/worktrees/{session}",
        GUEST_USER.home
    )
}

/// All of `stdout`, read to its end so the program never blocks on a full
/// pipe, of which only the last [`MAX_COMMAND_OUTPUT_BYTES`] are kept.
async fn read_tail(mut stdout: Pin<Box<dyn tokio::io::AsyncRead + Send>>) -> Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut chunk = vec![0; 8 * 1024];
    loop {
        let read = stdout
            .read(&mut chunk)
            .await
            .map_err(|error| HarnessError::Container(error.to_string()))?;
        if read == 0 {
            return Ok(kept);
        }
        kept.extend_from_slice(&chunk[..read]);
        if kept.len() > 2 * MAX_COMMAND_OUTPUT_BYTES {
            kept.drain(..kept.len() - MAX_COMMAND_OUTPUT_BYTES);
        }
    }
}
