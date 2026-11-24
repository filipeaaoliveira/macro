//! The `plan_review` and `split` Steps: before a person reads the Run's
//! plan at its Gate, the factory checks the plan's size block
//! ([`PlanSize`]), then a plan reviewer, in a session of its own, reads the
//! plan and hands back the Blockers it found.
//!
//! A Blocker the mechanical checks find ends the Step `blockers` without
//! asking the reviewer: the planner revises, and the next visit checks the
//! revision. So does a reviewer's Blocker. At the entry's last visit, a plan
//! that still has Blockers ends `reviewed` with them, as a plan that passed
//! does: the `code` Flow's plan Gate opens after `reviewed`, so a person
//! (or a rule, or the stand-in) reads the review's result beside the plan.
//!
//! A plan whose size block says the task is too big for one Run ends
//! `oversized`, when its entry routes it, and goes to `split`, which files
//! each child task as the Run's owner, in the project of the Run's task,
//! and ends `done` with their links: the `code` Flow then ends the Run for
//! a person. Each child's id is fixed by the Run and the Step, so a retried
//! `split` files no second task. Each child filed is kept with its id and
//! name (`RunRepository::record_split_task`), so the Run's read lists them
//! (`RunDetail::split_tasks`) without parsing the Step's output.

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::models::Outcome;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use serde::Deserialize;

use super::checks_round::{fit, records};
use super::{BuiltInStepError, BuiltInSteps, outcomes};
use crate::domain::control_files::ControlFiles;
use crate::domain::flow::StepName;
use crate::domain::models::{StepInput, StepReport};
use crate::domain::plan_review::{
    BLOCKERS_LEFT, PlanSize, child_task, child_task_id, control_file_flags, done_when_items,
    latest_plan, misplaced_files, task_mention, unmapped_items,
};
use crate::domain::ports::{ChildTasks, FollowUpMade, MainWatches, PullRequests, StepAgent};
use crate::domain::review_rounds::json_block;

/// What the plan reviewer is told.
const PLAN_REVIEW_INSTRUCTIONS: &str = include_str!("plan_review.md");

/// The plan reviewer's outcome for a plan it finds no Blocker in.
const PASS: &str = "pass";

/// The outcome of a plan with Blockers, which goes back to the planner;
/// also the plan reviewer's, for a plan it found some in.
const BLOCKERS: &str = "blockers";

/// The outcome of a plan whose review is done: it passed, or it is the
/// entry's last visit. The plan Gate opens after it.
const REVIEWED: &str = "reviewed";

/// The outcome of a plan that splits the task into child tasks.
const OVERSIZED: &str = "oversized";

/// How many rounds the review runs before the plan goes to its Gate with
/// the Blockers left, when its entry sets no bound.
const DEFAULT_ROUNDS: u16 = 3;

/// The most bytes of one Blocker's text the report keeps.
const MAX_BLOCKER_BYTES: usize = 2048;

/// A Blocker the plan reviewer wrote.
#[derive(Deserialize)]
struct WrittenBlocker {
    problem: String,
    #[serde(default)]
    why: String,
}

/// The block the plan reviewer ends its text with.
#[derive(Deserialize)]
struct WrittenBlockers {
    blockers: Vec<WrittenBlocker>,
}

/// What the plan reviewer handed back.
struct PlanReviewerAnswer {
    /// Whether it passed the plan.
    passed: bool,
    /// Its text, without its block of Blockers.
    summary: String,
    /// Its Blockers.
    blockers: Vec<String>,
}

/// What the mechanical checks found of a plan.
#[derive(Default)]
struct Mechanical {
    /// What must change before the plan can be carried out.
    blockers: Vec<String>,
    /// What the plan reviewer and the person at the Gate should see.
    flags: Vec<String>,
}

impl<A, G, R, M, C> BuiltInSteps<A, G, R, M, C>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository + ReviewRoundRepository,
    M: MainWatches,
    C: ChildTasks,
{
    /// Check the Run's latest plan, then have the plan reviewer read it.
    pub(super) async fn review_plan(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        if input.repository.is_none() {
            return Err(BuiltInStepError::NoRepository(step.clone()));
        }
        let Some(plan) = latest_plan(input.earlier) else {
            return Ok(StepReport {
                outcome: Outcome::failed(),
                output:
                    "The run has no plan to review: no `plan` step ended `done` before this one."
                        .to_string(),
            });
        };

        let size = PlanSize::from_plan(plan);
        if let Ok(PlanSize::Oversized { why, children }) = &size
            && input.routes(OVERSIZED)
        {
            let listed: Vec<String> = children
                .iter()
                .map(|child| format!("- {}", child.name))
                .collect();
            return Ok(StepReport {
                outcome: super::accept::outcome(OVERSIZED),
                output: format!(
                    "The plan finds the task too big for one run ({why}): it is split into {} child tasks.\n\n{}",
                    children.len(),
                    listed.join("\n")
                ),
            });
        }

        let mechanical = self.check_plan(input, size).await?;
        // A Flow whose review entry sets no bound still puts the plan to
        // its Gate, after the rounds the `code` Flow gives it.
        let last_visit = self.at_last_visit(input, Some(DEFAULT_ROUNDS)).await?;
        if !mechanical.blockers.is_empty() {
            return Ok(verdict(&mechanical, None, last_visit));
        }

        let instructions = review_instructions(&mechanical.flags);
        let answer = self
            .ask_agent(step, &instructions, outcomes(&[PASS, BLOCKERS]), input)
            .await?;
        // A question goes to the Run's owner, as does a reviewer that
        // stopped without a result: neither is a verdict on the plan.
        if answer.outcome == Outcome::question() || answer.outcome == Outcome::no_result() {
            return Ok(answer);
        }
        let reviewer = read_answer(answer.outcome.as_str() == PASS, &answer.output);
        Ok(verdict(&mechanical, Some(&reviewer), last_visit))
    }

    /// What the mechanical checks find of the plan whose size block says
    /// `size`: a block that does not read, or an oversized plan its entry
    /// sends nowhere, is a Blocker; a plan that fits has its files read at
    /// the Run's base, its Done-when mapping matched to the request's, and
    /// its Control Files flagged.
    async fn check_plan(
        &self,
        input: StepInput<'_>,
        size: Result<PlanSize, crate::domain::plan_review::SizeBlockError>,
    ) -> Result<Mechanical, BuiltInStepError> {
        let (files, done_when) = match size {
            Ok(PlanSize::Fits { files, done_when }) => (files, done_when),
            Ok(PlanSize::Oversized { .. }) => {
                return Ok(Mechanical {
                    blockers: vec![
                        "The plan finds the task too big for one run, and this run's flow splits no task: \
                         plan a change that fits, or say in the plan what to leave to a later task."
                            .to_string(),
                    ],
                    flags: Vec::new(),
                });
            }
            Err(error) => {
                return Ok(Mechanical {
                    blockers: vec![format!("The size block does not hold: {error}.")],
                    flags: Vec::new(),
                });
            }
        };

        let mut mechanical = Mechanical::default();
        let items = done_when_items(input.request);
        if items.is_empty() {
            mechanical.flags.push(
                "The request lists no Done-when items: the plan reviewer checks the plan does what it asks."
                    .to_string(),
            );
        }
        mechanical
            .blockers
            .extend(unmapped_items(&items, &done_when));

        let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
        let at_base = self.pull_requests.base_files(&input, &paths).await?;
        mechanical
            .blockers
            .extend(misplaced_files(&files, &at_base));

        let control_files = match input.control_file_patterns {
            Some(patterns) => ControlFiles::parse(patterns),
            None => ControlFiles::parse(&ControlFiles::default_patterns()),
        };
        // A Run's kept patterns were checked when it was created: one that
        // does not read flags nothing here, as the pull request Steps fail
        // on it later.
        if let Ok(control_files) = control_files {
            mechanical.flags.extend(control_file_flags(
                &files,
                &control_files,
                input.request,
                input.created_in_person,
            ));
        }
        Ok(mechanical)
    }

    /// File the child tasks of the Run's oversized plan, as its owner, and
    /// end `done` with their links.
    pub(super) async fn split_task(
        &self,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        let size = latest_plan(input.earlier).map(PlanSize::from_plan);
        let Some(Ok(PlanSize::Oversized { why, children })) = size else {
            return Ok(StepReport {
                outcome: Outcome::failed(),
                output: "The run's latest plan names no child tasks to split the task into."
                    .to_string(),
            });
        };

        let unavailable =
            |error| BuiltInStepError::ChildTask(rootcause::Report::new(error).into_dynamic());
        // Each child links the task it was split from by its name, as the
        // owner sees it.
        let parent = match input.task_id {
            Some(task_id) => {
                let name = self
                    .child_tasks
                    .task_name(input.owner, task_id)
                    .await
                    .map_err(unavailable)?;
                Some((
                    task_id,
                    name.unwrap_or_else(|| "the run's task".to_string()),
                ))
            }
            None => None,
        };

        let mut filed = Vec::new();
        let mut refused = Vec::new();
        for (index, child) in children.iter().enumerate() {
            // A retry of a Run that failed here files its children under
            // the retried Run's ids, at the same place, so the ones filed
            // before it failed are not filed twice.
            let task_id = child_task_id(input.branch_run(), input.key.seq, index);
            let of = parent.as_ref().map(|(id, name)| (*id, name.as_str()));
            let task = child_task(child, of, input.key.run_id);
            let made = self
                .child_tasks
                .file_child(input.owner, input.task_id, &task, task_id)
                .await
                .map_err(unavailable)?;
            match made {
                Some(FollowUpMade::Made) => {
                    // Kept for the Run's read, which lists the children
                    // without asking document storage. A failed write is
                    // a records error, which is retried: the retry files
                    // the same ids, which document storage finds made,
                    // and the write does nothing for a child kept already.
                    let place = u16::try_from(index).unwrap_or(u16::MAX);
                    self.pull_requests
                        .records()
                        .record_split_task(input.key, place, task_id, &child.name)
                        .await
                        .map_err(records)?;
                    filed.push(task_mention(task_id, &child.name));
                }
                // A task made under another id would be made again by every
                // retry: a fault to stop at, as for a follow-up task.
                Some(FollowUpMade::OtherId(made)) => {
                    return Err(BuiltInStepError::ChildTaskElsewhere(format!(
                        "document storage made child task {task_id} as {made}"
                    )));
                }
                Some(FollowUpMade::Refused) => refused.push(child.name.clone()),
                None => {
                    return Ok(StepReport {
                        outcome: Outcome::failed(),
                        output: "The plan splits the task, and this factory files no tasks: \
                                 split it by hand, as the plan says."
                            .to_string(),
                    });
                }
            }
        }

        let mut output = format!(
            "The task is too big for one run ({why}): its plan split it into {} child tasks, \
             filed as the run's owner. The run ends here; start a run on each child task.\n",
            children.len()
        );
        for mention in &filed {
            output.push_str(&format!("\n- {mention}"));
        }
        if !refused.is_empty() {
            output.push_str("\n\nDocument storage refused these, so file them by hand:\n");
            for name in &refused {
                output.push_str(&format!("\n- {name}"));
            }
        }
        Ok(StepReport {
            outcome: Outcome::done(),
            output,
        })
    }
}

/// What the plan reviewer is told: the Step's instructions, then what the
/// mechanical checks flagged.
fn review_instructions(flags: &[String]) -> String {
    if flags.is_empty() {
        return PLAN_REVIEW_INSTRUCTIONS.to_string();
    }
    let flags: Vec<String> = flags.iter().map(|flag| format!("- {flag}")).collect();
    format!(
        "{PLAN_REVIEW_INSTRUCTIONS}\n## What the factory's checks flagged\n\n{}\n",
        flags.join("\n")
    )
}

/// The plan reviewer's answer: `passed` as its outcome says, and the
/// Blockers its block lists. One that asks for changes and lists none
/// that read has its text as its one Blocker, so a revision still reads
/// it.
fn read_answer(passed: bool, text: &str) -> PlanReviewerAnswer {
    let (summary, listed) = match json_block(text) {
        Some((summary, block)) => {
            let blockers = serde_json::from_str::<WrittenBlockers>(block)
                .map(|written| written.blockers)
                .unwrap_or_default();
            (summary, blockers)
        }
        None => (text.trim().to_string(), Vec::new()),
    };
    let mut blockers: Vec<String> = listed
        .into_iter()
        .filter(|blocker| !blocker.problem.trim().is_empty())
        .map(|blocker| {
            let line = match blocker.why.trim() {
                "" => blocker.problem.trim().to_string(),
                why => format!("{}: {why}", blocker.problem.trim()),
            };
            super::cut(&line.replace('\n', " "), MAX_BLOCKER_BYTES).to_string()
        })
        .collect();
    if !passed && blockers.is_empty() {
        blockers.push("The plan reviewer asked for changes: see its report.".to_string());
    }
    PlanReviewerAnswer {
        passed: passed && blockers.is_empty(),
        summary,
        blockers,
    }
}

/// How the Step ends: `reviewed` when the mechanical checks and the plan
/// reviewer found no Blocker, or at the entry's `last_visit`; `blockers`
/// otherwise. Its output says why, then what the checks found, then the
/// reviewer's report, when it was asked.
fn verdict(
    mechanical: &Mechanical,
    reviewer: Option<&PlanReviewerAnswer>,
    last_visit: bool,
) -> StepReport {
    let reviewer_blockers = reviewer.map_or(0, |answer| answer.blockers.len());
    let count = mechanical.blockers.len() + reviewer_blockers;
    let passed = count == 0 && reviewer.is_some_and(|answer| answer.passed);

    let summary = if passed {
        "The plan review passed: the factory's checks and the plan reviewer found no blocker."
            .to_string()
    } else if last_visit {
        format!("{BLOCKERS_LEFT}\n\nIts last round found {count} blockers.")
    } else {
        format!("The plan review found {count} blockers: the planner revises the plan.")
    };

    let mut body = String::from("## The factory's checks\n\n");
    if mechanical.blockers.is_empty() {
        body.push_str("No blocker: the plan's files and its Done-when mapping hold.\n");
    } else {
        for blocker in &mechanical.blockers {
            body.push_str(&format!("- Blocker: {blocker}\n"));
        }
    }
    for flag in &mechanical.flags {
        body.push_str(&format!("- Flag: {flag}\n"));
    }
    match reviewer {
        Some(answer) => {
            body.push_str(&format!(
                "\n## The plan reviewer's report\n\n{}\n",
                answer.summary
            ));
            for (index, blocker) in answer.blockers.iter().enumerate() {
                body.push_str(&format!("\n{}. Blocker: {blocker}", index + 1));
            }
        }
        None => body.push_str(
            "\nThe plan reviewer was not asked: the plan is revised for the checks' blockers first.\n",
        ),
    }

    let ended = if passed || last_visit {
        REVIEWED
    } else {
        BLOCKERS
    };
    StepReport {
        outcome: super::accept::outcome(ended),
        output: fit(&summary, &body),
    }
}
