//! Acceptance checks: what the `plan` Step writes for each Done-when clause
//! of the Run's request, so the `accept` Step can check the change does
//! what the request asks before the merge.
//!
//! The planner ends its plan with a fenced `json` block:
//!
//! ```json
//! {"acceptance": [
//!   {"clause": "A merged run ends", "run": "cargo test -p app merged_run_ends"},
//!   {"clause": "The page shows the new badge", "none": "only a browser shows it"}
//! ]}
//! ```
//!
//! Each clause has either a check, `run`, a command run from the
//! repository's root whose exit status says whether the clause holds (with
//! an optional `timeout` in seconds, as a repository check's), or `none`,
//! why it has no check: the verifier then checks it by hand. `accept` runs
//! every check on the head of the Run's branch.
//!
//! The `plan` Step reads the block, and keeps the plan with it written out
//! for the person at its Gate, then the block as the factory read it, last
//! ([`plan_output`]). The factory keeps the checks in the plan Step's
//! recorded output, never in the Run's branch: the writer and fixer push
//! the branch, and cannot change the commands. Only the planner writes
//! them.
//!
//! Trust: the commands are an agent's, not people's. A repository's
//! `.macro/checks.json` is written and merged by people; acceptance checks
//! are written by the planner, and run in the verifier's session, on its
//! machine, with the same trust as any agent's command there. The plan's
//! Gate shows them, but a Standing Rule or the stand-in may answer that
//! Gate, so they may run without a person having seen them. And what a
//! command runs may be code the writer wrote, such as a test it adds: the
//! command is fixed, its target is not. The verifier is told to read what
//! each check runs and confirm it really tests its clause; a check that
//! passes proves only as much as the code it runs.

#[cfg(test)]
mod test;

use factory_records::domain::models::{MAX_OUTPUT_BYTES, Step};
use serde::{Deserialize, Serialize};

use crate::domain::built_in_steps::cut;
use crate::domain::checks::{DEFAULT_CHECK_TIMEOUT_SECS, MAX_CHECK_TIMEOUT_SECS};
use crate::domain::review_rounds::{escape, json_block};

/// The entry whose Step's output holds the acceptance checks.
const PLAN_STEP: &str = "plan";

/// The most clauses a plan's acceptance checks hold: with their commands,
/// what a round of checks records stays within what it keeps.
const MAX_CLAUSES: usize = 16;

/// The longest clause or reason, in bytes.
const MAX_TEXT_BYTES: usize = 2048;

/// The longest command, in bytes: with the variables a check is told, it
/// stays within what a check runs.
const MAX_RUN_BYTES: usize = 1024;

/// Why a check a recorded plan marked `deployed` is checked by hand: it
/// needed the deployed change, and the factory ends a Run at its merge.
const DEPLOYED_BY_HAND: &str = "its check needed the deployed change, which the factory no longer \
     waits for: check it as far as the branch shows";

/// What opens and closes a fenced block.
const FENCE: &str = "```";

/// The most bytes the acceptance checks take in the plan's output, written
/// out and as their block.
const MAX_NOTE_BYTES: usize = 16 * 1024;

/// What ends a plan cut to leave room for its acceptance checks.
const CUT_NOTE: &str = "\n\n(The rest of the plan is left out: a step's output holds 64 KiB.)";

/// One Done-when clause, and how it is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acceptance {
    /// The clause, as the planner quoted it from the request.
    pub clause: String,
    /// How it is checked.
    pub check: AcceptanceCheck,
}

/// How a clause is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptanceCheck {
    /// A command run from the repository's root, which exits 0 when the
    /// clause holds, within `timeout` seconds.
    Run {
        /// The command.
        run: String,
        /// How many seconds it may take.
        timeout: u64,
    },
    /// The clause has no check, for this reason: the verifier checks it.
    None(String),
}

/// A plan's acceptance checks, one per Done-when clause.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AcceptanceChecks {
    /// The clauses, in the plan's order.
    pub clauses: Vec<Acceptance>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WrittenChecks {
    acceptance: Vec<WrittenClause>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WrittenClause {
    clause: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    none: Option<String>,
    /// Read, never written: a plan recorded before Runs ended at their
    /// merge may mark a check as one of the deployed change, which nothing
    /// deploys now. Such a check is checked by hand.
    #[serde(default, skip_serializing)]
    deployed: bool,
}

/// Why a plan's acceptance checks do not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AcceptanceError {
    /// The block is not the acceptance checks' JSON.
    #[error("the acceptance block does not read: {0}")]
    Unreadable(String),
    /// It lists no clause, or more than a plan keeps.
    #[error("the acceptance block lists {0} clauses; a plan lists 1 to {MAX_CLAUSES}")]
    Clauses(usize),
    /// A clause is blank, too long, or holds a code fence.
    #[error(
        "clause {0} of the acceptance block is blank, over {MAX_TEXT_BYTES} bytes, or holds a code fence"
    )]
    Clause(usize),
    /// A clause has both a check and a reason for none, or neither, or one
    /// that is blank or too long.
    #[error(
        "clause {0} of the acceptance block must have either a command (`run`, at most {MAX_RUN_BYTES} bytes) or why it has none (`none`, at most {MAX_TEXT_BYTES} bytes), not blank and with no code fence"
    )]
    Check(usize),
    /// A clause's timeout is outside what a check may take.
    #[error(
        "clause {0} of the acceptance block has a timeout outside 1 to {MAX_CHECK_TIMEOUT_SECS} seconds"
    )]
    Timeout(usize),
    /// The checks take more of the plan's output than they may.
    #[error("the acceptance checks are over {MAX_NOTE_BYTES} bytes")]
    TooLarge,
}

impl AcceptanceChecks {
    /// The acceptance checks a plan's output ends with, in its last fenced
    /// `json` block; none when it has no block naming `acceptance`.
    pub fn from_plan(output: &str) -> Result<Option<Self>, AcceptanceError> {
        Ok(Self::split(output)?.map(|(_, checks)| checks))
    }

    /// The plan's text without its acceptance block, and the checks the
    /// block holds; none when it has no block naming `acceptance`.
    fn split(output: &str) -> Result<Option<(String, Self)>, AcceptanceError> {
        let Some((plan, block)) = json_block(output) else {
            return Ok(None);
        };
        let value: serde_json::Value = match serde_json::from_str(block) {
            Ok(value) => value,
            Err(error) => return Err(AcceptanceError::Unreadable(error.to_string())),
        };
        if value.get("acceptance").is_none() {
            return Ok(None);
        }
        let written: WrittenChecks = serde_json::from_value(value)
            .map_err(|error| AcceptanceError::Unreadable(error.to_string()))?;
        if written.acceptance.is_empty() || written.acceptance.len() > MAX_CLAUSES {
            return Err(AcceptanceError::Clauses(written.acceptance.len()));
        }

        let mut clauses = Vec::new();
        for (index, written) in written.acceptance.into_iter().enumerate() {
            let number = index + 1;
            let clause = written.clause.trim().to_string();
            // A fence in any of them would close the block the factory
            // keeps them in.
            if clause.is_empty() || clause.len() > MAX_TEXT_BYTES || clause.contains(FENCE) {
                return Err(AcceptanceError::Clause(number));
            }
            let usable = |text: &str, most: usize| {
                !text.trim().is_empty() && text.len() <= most && !text.contains(FENCE)
            };
            let check = match (written.run, written.none, written.timeout) {
                (Some(run), None, timeout) if usable(&run, MAX_RUN_BYTES) => {
                    let timeout = timeout.unwrap_or(DEFAULT_CHECK_TIMEOUT_SECS);
                    if timeout == 0 || timeout > MAX_CHECK_TIMEOUT_SECS {
                        return Err(AcceptanceError::Timeout(number));
                    }
                    if written.deployed {
                        AcceptanceCheck::None(DEPLOYED_BY_HAND.to_string())
                    } else {
                        AcceptanceCheck::Run { run, timeout }
                    }
                }
                (None, Some(why), None) if usable(&why, MAX_TEXT_BYTES) => {
                    AcceptanceCheck::None(why.trim().to_string())
                }
                _ => return Err(AcceptanceError::Check(number)),
            };
            clauses.push(Acceptance { clause, check });
        }
        Ok(Some((plan, Self { clauses })))
    }

    /// The acceptance checks of the Run's plan: those of its latest `plan`
    /// Step that ended, among `earlier`, the plan a person approved when
    /// the plan has a Gate. None when there is no plan, or it names none;
    /// why, when they do not read.
    pub fn of_run(earlier: &[Step]) -> Result<Option<Self>, AcceptanceError> {
        let plan = earlier
            .iter()
            .rev()
            .find(|step| step.entry.as_str() == PLAN_STEP && step.end.is_some());
        let Some(end) = plan.and_then(|plan| plan.end.as_ref()) else {
            return Ok(None);
        };
        // Checks too large to keep were dropped with their block: the note
        // the plan kept in their place says so.
        match Self::from_plan(&end.output)? {
            None if end
                .output
                .contains(&unreadable_note(&AcceptanceError::TooLarge)) =>
            {
                Err(AcceptanceError::TooLarge)
            }
            checks => Ok(checks),
        }
    }

    /// The checks as a person reads them at the plan's Gate: each clause,
    /// numbered, with its command or why it has none. Escaped, as the
    /// planner wrote them.
    pub fn render(&self) -> String {
        let mut text = String::new();
        for (index, acceptance) in self.clauses.iter().enumerate() {
            let clause = escape(&acceptance.clause).replace('\n', " ");
            let how = match &acceptance.check {
                AcceptanceCheck::Run { run, .. } => {
                    format!("checked by `{}`", run.replace('`', "'").replace('\n', " "))
                }
                AcceptanceCheck::None(why) => {
                    format!("no check: {}", escape(why).replace('\n', " "))
                }
            };
            text.push_str(&format!("{}. {clause}: {how}\n", index + 1));
        }
        text
    }
}

impl AcceptanceChecks {
    /// The checks as the plan's fenced `json` block, as the factory read
    /// them.
    fn block(&self) -> String {
        let written = WrittenChecks {
            acceptance: self
                .clauses
                .iter()
                .map(|acceptance| {
                    let (run, timeout, none) = match &acceptance.check {
                        AcceptanceCheck::Run { run, timeout } => {
                            (Some(run.clone()), Some(*timeout), None)
                        }
                        AcceptanceCheck::None(why) => (None, None, Some(why.clone())),
                    };
                    WrittenClause {
                        clause: acceptance.clause.clone(),
                        run,
                        timeout,
                        none,
                        deployed: false,
                    }
                })
                .collect(),
        };
        let json = serde_json::to_string_pretty(&written).unwrap_or_default();
        format!("```json\n{json}\n```")
    }
}

/// What the `plan` Step keeps of a plan, `output`, so the person at its
/// Gate sees the acceptance checks the factory keeps, or that it keeps
/// none and why: the plan without its acceptance block (cut to leave room),
/// then the checks written out, then the block as the factory read it,
/// which the `accept` Step reads back.
pub fn plan_output(output: &str) -> String {
    let (plan, note) = match AcceptanceChecks::split(output) {
        Ok(Some((plan, checks))) => {
            let note = format!(
                "\n\n## Acceptance checks\n\nThe factory keeps these from the plan: the writer and fixer cannot \
                 change them. The verifier runs each check before the merge, and checks by hand each clause \
                 without one.\n\n{}\n{}",
                checks.render(),
                checks.block()
            );
            // Checks too large to keep are dropped with their block, so no
            // later Step reads them.
            if note.len() > MAX_NOTE_BYTES {
                (plan, unreadable_note(&AcceptanceError::TooLarge))
            } else {
                (plan, note)
            }
        }
        Ok(None) => (
            output.to_string(),
            "\n\n## Acceptance checks\n\nThe plan names none: the verifier checks the request's \
             Done-when clauses by hand."
                .to_string(),
        ),
        Err(error) => (output.to_string(), unreadable_note(&error)),
    };

    if plan.len() + note.len() <= MAX_OUTPUT_BYTES {
        return format!("{plan}{note}");
    }
    let room = MAX_OUTPUT_BYTES - note.len() - CUT_NOTE.len();
    format!("{}{CUT_NOTE}{note}", cut(&plan, room))
}

/// What the plan's Gate says of acceptance checks that do not read.
fn unreadable_note(error: &AcceptanceError) -> String {
    format!(
        "\n\n## Acceptance checks\n\nThe plan's acceptance checks do not read ({error}): the verifier \
         checks the request's Done-when clauses by hand. Send the plan back to have them written again."
    )
}
