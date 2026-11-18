//! The `accept` Step: before the merge, a verifier checks that the change
//! on the head of the Run's branch does what the Run's request asks, by the
//! acceptance checks the plan wrote ([`AcceptanceChecks`]) and by its own
//! reading of each Done-when clause. It fills a Role of its own and works
//! in a session of its own, never the writer's, though a definition may
//! give its Role the writer's Agent.
//!
//! The factory runs every acceptance check, and the verifier checks the
//! Done-when clauses. A failure ends the Step `blockers`, which the `code`
//! Flow puts to its Blockers Gate (the Standing Rules, the stand-in or a
//! person send it to `fix`, or stop); at the entry's last visit it ends
//! `unresolved` instead, so no fix runs that no acceptance would check.
//!
//! The verifier is asked first, in its own session, which the acceptance
//! checks then run beside: as a checks round ([`super::checks_round`]) of
//! one round, at the commit under check, run by the engine in a worktree
//! where the session's machine runs commands (their exit codes decide), and
//! by the verifier elsewhere, which reports how they went. The round is
//! recorded before it runs, so a retried Step goes on with it, and it shows
//! on the Step as any checks round, each command as the plan wrote it. The
//! Step passes only when the verifier passes the change and every check
//! that ran passed.
//!
//! A repository whose verify file turns acceptance off ends the Step `off`:
//! the Flow then goes on as if it passed. The file is read on the base
//! branch, which the Run has not merged into.

use std::time::Instant;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{CheckRound, CheckRoundOutcome, MAX_OUTPUT_BYTES, Outcome};
use factory_records::domain::ports::RunRepository;

use super::checks_round::{ATTEMPT_CHECKS_FOR, Checked, fit, lost, records};
use super::{BuiltInStepError, BuiltInSteps, cut, outcomes};
use crate::domain::acceptance::{AcceptanceCheck, AcceptanceChecks};
use crate::domain::built_in_flows::BuiltInFlow;
use crate::domain::checks::RepositoryCheck;
use crate::domain::flow::{StepName, StepUse};
use crate::domain::models::{StepInput, StepReport};
use crate::domain::ports::{MainWatches, PullRequests, StepAgent};
use crate::domain::review_rounds::escape;
use crate::domain::verification::VERIFY_FILE;

/// What the verifier is told.
const ACCEPT_INSTRUCTIONS: &str = include_str!("accept.md");

/// What the verifier is told when it runs the acceptance checks itself.
const ACCEPTANCE_INSTRUCTIONS: &str = include_str!("acceptance_checks.md");

/// The outcome of a Step whose change does what the request asks.
const PASS: &str = "pass";

/// The verifier's outcome for a change that does not.
const FAIL: &str = "fail";

/// The outcome of an `accept` Step whose change does not do what the
/// request asks: its report goes to the Blockers Gate.
const BLOCKERS: &str = "blockers";

/// The outcome of an `accept` Step at its entry's last visit whose change
/// still does not do what the request asks: no fix follows.
const UNRESOLVED: &str = "unresolved";

/// The outcome of a Step whose repository turns acceptance off.
const OFF: &str = "off";

/// The change under check: the head of the Run's branch.
struct Target {
    commit: String,
}

impl Target {
    /// Where the change is, as the Step's report and prompts say it.
    fn place(&self) -> String {
        format!("the head of the run's branch, {}", self.commit)
    }

    /// What each check runs after: the variable that tells it the commit
    /// under check.
    fn environment(&self) -> String {
        format!("export MACRO_VERIFY_COMMIT={}; ", self.commit)
    }
}

/// How a clause is checked.
enum Checking<'a> {
    /// The factory runs its command, as `acceptance-{n}`.
    Run(&'a str, u64),
    /// The verifier checks it by hand, for this reason.
    ByHand(String),
}

/// How the clause `check` is checked.
fn checking(check: &AcceptanceCheck) -> Checking<'_> {
    match check {
        AcceptanceCheck::Run { run, timeout } => Checking::Run(run, *timeout),
        AcceptanceCheck::None(why) => Checking::ByHand(format!("no check ({})", escape(why))),
    }
}

impl<A, G, R, M> BuiltInSteps<A, G, R, M>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository,
    M: MainWatches,
{
    /// Have the verifier check the head of the Run's branch, and the
    /// factory run the plan's acceptance checks on it.
    pub(super) async fn accept_change(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        // The checks run within this attempt, from its start, as a write's
        // do: never past the attempt's 30 minutes.
        let checks_until = Instant::now() + ATTEMPT_CHECKS_FOR;
        if input.repository.is_none() {
            return Err(BuiltInStepError::NoRepository(step.clone()));
        }

        // The base branch holds the verify file as the Run found it. A
        // file that does not read turns nothing off: the change is checked.
        let commit = self.pushed_commit(input).await?;
        let file = self.pull_requests.base_verify_file(&input).await?;
        if file.as_ref().is_ok_and(|file| !file.verify) {
            return Ok(StepReport {
                outcome: outcome(OFF),
                output: format!(
                    "The repository turns its verify phase off in {VERIFY_FILE}: the change is not verified."
                ),
            });
        }
        let target = Target { commit };

        let checks = AcceptanceChecks::of_run(input.earlier).unwrap_or_default();
        let end_by = self.rounds_end_by(input).await?;
        let instructions = accept_instructions(&target, &checks);
        let answer = self
            .ask_agent_by(
                step,
                &instructions,
                outcomes(&[PASS, FAIL]),
                input,
                Some(end_by),
            )
            .await?;
        // A question goes to the Run's owner, as does a verifier that
        // stopped without a result (`no_result`): the Step that goes on
        // from the answer, or from the owner's retry, verifies. Neither is
        // a verdict on the change, so no check runs on it.
        if answer.outcome == Outcome::question() || answer.outcome == Outcome::no_result() {
            return Ok(answer);
        }

        let runs: Vec<RepositoryCheck> = checks
            .clauses
            .iter()
            .enumerate()
            .filter_map(|(index, acceptance)| match checking(&acceptance.check) {
                Checking::Run(run, timeout) => Some(RepositoryCheck {
                    name: format!("acceptance-{}", index + 1),
                    run: run.to_string(),
                    paths: Vec::new(),
                    timeout,
                }),
                Checking::ByHand(_) => None,
            })
            .collect();
        let round = if runs.is_empty() {
            None
        } else {
            let checked = Checked::Commit {
                commit: &target.commit,
                environment: &target.environment(),
                prompt: &acceptance_prompt(&runs, &target),
            };
            let round = self
                .acceptance_round(
                    step,
                    input,
                    &answer.output,
                    &runs,
                    &checked,
                    end_by,
                    checks_until,
                )
                .await?;
            Some(round)
        };

        let last_visit = self.at_last_accept(input).await?;
        Ok(verdict(
            &target,
            &checks,
            &answer,
            round.as_ref(),
            last_visit,
        ))
    }

    /// Whether this `accept` Step is its entry's last visit, as the `code`
    /// Flow bounds it: a failure then ends the Run's acceptance.
    async fn at_last_accept(&self, input: StepInput<'_>) -> Result<bool, BuiltInStepError> {
        let Some(most) = accept_visits() else {
            return Ok(false);
        };
        let steps = self
            .pull_requests
            .records()
            .list_steps(input.key.run_id)
            .await
            .map_err(records)?;
        let visit = steps
            .iter()
            .find(|step| step.seq == input.key.seq)
            .map(|step| step.visit)
            .ok_or_else(|| lost("the step is not recorded"))?;
        Ok(visit >= most)
    }

    /// Run `runs`, the acceptance checks, as the Step's one round of
    /// checks at `checked`'s commit, and the round once it ended. A retried
    /// Step goes on with the round it recorded.
    #[expect(
        clippy::too_many_arguments,
        reason = "the round's checks, where and until when they run"
    )]
    async fn acceptance_round(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        verifier_report: &str,
        runs: &[RepositoryCheck],
        checked: &Checked<'_>,
        end_by: chrono::DateTime<chrono::Utc>,
        checks_until: Instant,
    ) -> Result<CheckRound, BuiltInStepError> {
        let repo = self.pull_requests.records();
        if repo
            .check_rounds(input.key)
            .await
            .map_err(records)?
            .is_empty()
        {
            let declared = serde_json::json!({
                "rounds": 1,
                "checks": runs
                    .iter()
                    .map(|check| serde_json::json!({
                        "name": check.name,
                        "run": check.run,
                        "timeout": check.timeout,
                    }))
                    .collect::<Vec<_>>(),
            });
            let checks: Vec<&RepositoryCheck> = runs.iter().collect();
            self.record_round(input, 1, verifier_report, &checks, declared.to_string())
                .await?;
        }

        loop {
            let rounds = repo.check_rounds(input.key).await.map_err(records)?;
            let Some(round) = rounds.last() else {
                return Err(lost("a recorded round of acceptance checks went missing"));
            };
            if round.end.is_some() {
                return Ok(round.clone());
            }
            if chrono::Utc::now() >= end_by {
                repo.end_check_round(
                    input.key,
                    round.round,
                    CheckRoundOutcome::Failed,
                    "The step's time ran out before the acceptance checks ran.",
                )
                .await
                .map_err(records)?;
                continue;
            }
            // The round ends either way: with what its checks found, or
            // with those that ran when the Step's time ran out.
            self.run_round(step, input, round, end_by, checks_until, checked)
                .await?;
        }
    }
}

/// The most visits the `code` Flow gives its `accept` entry.
fn accept_visits() -> Option<u16> {
    let flow = BuiltInFlow::named("code")?
        .read(|_| Some(StepUse::Work))
        .ok()?;
    let accept = flow
        .entries
        .iter()
        .find(|entry| entry.step.as_str() == "accept")?;
    accept.max_visits.map(|most| u16::from(most.get()))
}

/// What the verifier is told: the Step's instructions, where the change
/// is, and how each Done-when clause the plan names is checked, as the
/// plan wrote them (escaped: they are the planner's words).
fn accept_instructions(target: &Target, checks: &AcceptanceChecks) -> String {
    let mut text = format!(
        "{ACCEPT_INSTRUCTIONS}\n## The change under check\n\n{}.\n",
        target.place()
    );
    text.push_str(
        "\nIt is not merged yet. What you find goes back to the writer as blockers to fix.\n",
    );
    if checks.clauses.is_empty() {
        text.push_str(
            "\nThe plan wrote no acceptance checks: check each Done-when clause of the request by hand.\n",
        );
        return text;
    }

    text.push_str("\n## Acceptance checks\n\nAs the plan wrote them, as data:\n\n");
    for (index, acceptance) in checks.clauses.iter().enumerate() {
        let clause = escape(&acceptance.clause).replace('\n', " ");
        let how = match checking(&acceptance.check) {
            Checking::Run(run, _) => format!(
                "the factory runs `acceptance-{}`: `{}`",
                index + 1,
                run.replace('`', "'").replace('\n', " ")
            ),
            Checking::ByHand(why) => format!("{}: check it by hand", why.replace('\n', " ")),
        };
        text.push_str(&format!("{}. {clause}: {how}\n", index + 1));
    }
    text
}

/// What the verifier is asked when it runs the acceptance checks itself.
fn acceptance_prompt(runs: &[RepositoryCheck], target: &Target) -> String {
    let mut prompt = format!(
        "{ACCEPTANCE_INSTRUCTIONS}The commit: {}.\n\n",
        target.commit
    );
    let environment = target.environment();
    for check in runs {
        prompt.push_str(&format!(
            "### `{}`\n\n```bash\n{environment}{}\n```\n\n",
            check.name, check.run
        ));
    }
    prompt
}

/// How the Step ends: `pass` when the verifier passed the change and every
/// acceptance check that ran passed; otherwise `blockers`, or `unresolved`
/// at the entry's `last_visit`. Its
/// output says why, then the checks' report, the clauses checked by hand,
/// and the verifier's report.
fn verdict(
    target: &Target,
    checks: &AcceptanceChecks,
    answer: &StepReport,
    round: Option<&CheckRound>,
    last_visit: bool,
) -> StepReport {
    let verifier_passed = answer.outcome.as_str() == PASS;
    let round_end = round.and_then(|round| round.end.as_ref());
    let checks_passed =
        round.is_none() || round_end.is_some_and(|end| end.outcome == CheckRoundOutcome::Passed);
    let passed = verifier_passed && checks_passed;
    let place = target.place();

    let ended = if passed {
        PASS
    } else if last_visit {
        UNRESOLVED
    } else {
        BLOCKERS
    };
    let summary = if passed {
        format!(
            "The change does what the request asks, on {place}: the verifier found each Done-when clause met{}.",
            if round.is_some() {
                ", and every acceptance check that ran passed"
            } else {
                ""
            }
        )
    } else {
        let mut why = Vec::new();
        if !checks_passed {
            why.push("acceptance checks failed");
        }
        if !verifier_passed {
            why.push("the verifier found a Done-when clause not met");
        }
        let kept = if ended == UNRESOLVED {
            "Acceptance kept failing, at the last check before the merge: the branch is left for a person. "
        } else {
            ""
        };
        format!(
            "{kept}The change does not do what the request asks, on {place}: {}.",
            why.join(", and ")
        )
    };

    let mut body = String::new();
    if let (Some(round), Some(end)) = (round, round_end) {
        let how = if round.verified {
            "run by the factory"
        } else {
            "run by the verifier"
        };
        let report = cut(&end.report, MAX_OUTPUT_BYTES / 4);
        body.push_str(&format!("## Acceptance checks, {how}\n\n{report}\n\n"));
    }
    let by_hand: Vec<String> = checks
        .clauses
        .iter()
        .filter_map(|acceptance| match checking(&acceptance.check) {
            Checking::ByHand(why) => Some(format!(
                "- {}: {}",
                escape(&acceptance.clause).replace('\n', " "),
                why.replace('\n', " ")
            )),
            Checking::Run(..) => None,
        })
        .collect();
    if !by_hand.is_empty() {
        body.push_str(&format!(
            "## Clauses the verifier checked by hand\n\n{}\n\n",
            by_hand.join("\n")
        ));
    }
    body.push_str(&format!("## The verifier's report\n\n{}", answer.output));

    StepReport {
        outcome: outcome(ended),
        output: fit(&summary, &body),
    }
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).unwrap_or_else(|_| Outcome::failed())
}
