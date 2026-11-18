//! The task thread: a Run's main updates, posted as its owner to one
//! comment thread on its task, when the task thread section of its
//! Orchestrator Definition is on (`task_thread`, off by default).
//!
//! The Run's start is the thread's first comment; each later update is one
//! reply: each Step's end (a pull request Step's names its pull request,
//! and the merge's the merged one), a Gate that waits for a person, a
//! Gate's answer and who or what gave it (the owner, a Standing Rule or the
//! stand-in), a notify-only Gate's notice, and the Run's end. One thread
//! per Run keeps the task's comments to one discussion a Run, however many
//! Steps it takes.
//!
//! Every comment is signed as the factory's ([`factory_signed`]), so the
//! owner's notes leave it out. Each has an id derived from its Run, its
//! update and when the update happened, so posting it again, as a retried
//! activity does, is refused as a conflict and posts nothing more. Posting
//! never fails or holds a Run: an update that cannot be posted within
//! [`POST_TIMEOUT`] is logged and dropped.

#[cfg(test)]
mod test;

use std::time::Duration;

use chrono::{DateTime, Utc};
use factory_records::domain::models::{GateAnswer, GateKind, Run, RunId, Step};
use factory_records::domain::ports::RunRepository;
use model_notifications::FactoryGateWaitingMetadata;
use uuid::Uuid;

use crate::domain::gate_owner::factory_signed;
use crate::domain::ports::{TaskThread, ThreadPosted};
use crate::domain::review_rounds::escape;

/// The namespace an update's comment id takes its fixed bytes from, by its
/// Run and its update. Fixed forever.
const THREAD_NAMESPACE: Uuid = Uuid::from_u128(0x6661_6374_6f72_792d_7468_7265_6164_7321);

/// How long posting one update may take, its thread's first comment
/// included, before it is dropped: the activity that posts it records the
/// Run first and must not wait on its comments.
pub const POST_TIMEOUT: Duration = Duration::from_secs(15);

/// The outcomes of the pull request Steps whose output names the pull
/// request: `publish`'s and `merge`'s.
const PULL_REQUEST_OUTCOMES: [&str; 2] = ["published", "merged"];

/// What happened in a Run that its task's thread is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunUpdate {
    /// The Run started: its first Step started.
    Started,
    /// The Step `seq` ended.
    StepEnded {
        /// Its place among the Run's Steps.
        seq: u16,
    },
    /// The Gate of the Step `seq` waits for a person.
    GateWaiting {
        /// The place of the Step that asks.
        seq: u16,
    },
    /// The Gate of the Step `seq` was answered.
    GateAnswered {
        /// The place of the Step that asked.
        seq: u16,
    },
    /// The notify-only Gate of the Step `seq` told the person `question`;
    /// the Run went on.
    GateTold {
        /// The place of the Step whose Gate tells.
        seq: u16,
        /// What it tells.
        question: String,
    },
    /// The Run ended.
    Ended,
}

/// A comment the factory posts on a Run's task, as its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadComment {
    /// Its id: the same every time the same update is posted.
    pub id: Uuid,
    /// The thread it replies to, by its first comment's id; none for the
    /// first comment.
    pub thread_id: Option<Uuid>,
    /// What it says, signed as the factory's.
    pub text: String,
}

/// Post `update` of the Run `run_id` to its task's thread, when its task
/// thread section is on and it works on a task. Never fails: what cannot
/// be read or posted is logged and dropped, so a Run never waits on it.
pub async fn post_update<R, P>(repo: &R, thread: &P, run_id: RunId, update: RunUpdate)
where
    R: RunRepository,
    P: TaskThread,
{
    let posting = post(repo, thread, run_id, &update);
    if tokio::time::timeout(POST_TIMEOUT, posting).await.is_err() {
        tracing::warn!(%run_id, ?update, "a run update was not posted in time");
    }
}

/// [`post_update`], without its time limit.
async fn post<R, P>(repo: &R, thread: &P, run_id: RunId, update: &RunUpdate)
where
    R: RunRepository,
    P: TaskThread,
{
    let run = match repo.get_run(run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(?error, %run_id, "a run update was not posted: the run did not read");
            return;
        }
    };
    let on = run
        .standing_rules
        .is_some_and(|rules| rules.definition.task_thread);
    let Some(task_id) = run.task_id.filter(|_| on) else {
        return;
    };
    let steps = match repo.list_steps(run_id).await {
        Ok(steps) => steps,
        Err(error) => {
            tracing::warn!(?error, %run_id, "a run update was not posted: its steps did not read");
            return;
        }
    };
    let Some(comment) = update_comment(&run, &steps, update) else {
        return;
    };

    // A reply whose thread is not there, as when the Run's first comment
    // was never posted, posts the first comment, then the reply again. A
    // task the owner no longer sees takes neither.
    let posted = thread.post(&run.user_id, task_id, &comment).await;
    let posted = match (posted, comment.thread_id) {
        (Ok(ThreadPosted::NoThread), Some(_)) => {
            match thread
                .post(&run.user_id, task_id, &first_comment(&run))
                .await
            {
                Ok(ThreadPosted::Posted) => thread.post(&run.user_id, task_id, &comment).await,
                other => other,
            }
        }
        (posted, _) => posted,
    };
    match posted {
        Ok(ThreadPosted::Posted) => {}
        Ok(ThreadPosted::NoThread) => {
            tracing::warn!(%run_id, %task_id, ?update, "a run update was not posted: the task is not there for its owner");
        }
        Err(error) => {
            tracing::warn!(?error, %run_id, %task_id, ?update, "a run update was not posted");
        }
    }
}

/// The id of the comment that posts the update `event` of the Run
/// `run_id`, which happened at `at`: a UUIDv7 of that time, as Macro takes
/// a comment's own id, whose other bytes are fixed by the Run and the
/// update, so every post of one update has the same id.
pub fn comment_id(run_id: RunId, event: &str, at: DateTime<Utc>) -> Uuid {
    let name = format!("{run_id}:{event}");
    let fixed = Uuid::new_v5(&THREAD_NAMESPACE, name.as_bytes());
    let mut bytes = [0u8; 10];
    bytes.copy_from_slice(&fixed.as_bytes()[6..16]);
    let millis = u64::try_from(at.timestamp_millis()).unwrap_or(0);
    uuid::Builder::from_unix_timestamp_millis(millis, &bytes).into_uuid()
}

/// The thread's first comment: the Run started.
pub fn first_comment(run: &Run) -> ThreadComment {
    let on = run
        .repository
        .as_ref()
        .map(|repository| format!(" on {} (`{}`)", repository.url, repository.base_branch))
        .unwrap_or_default();
    ThreadComment {
        id: comment_id(run.id, "start", run.created_at),
        thread_id: None,
        text: factory_signed(&format!(
            "**A factory run started**{on}. Its updates follow in this thread.\n\nRun `{}`.",
            run.id
        )),
    }
}

/// The comment that posts `update` of `run`, whose Steps are `steps`: none
/// when what it tells is not recorded, as a Step that has not ended.
pub fn update_comment(run: &Run, steps: &[Step], update: &RunUpdate) -> Option<ThreadComment> {
    let step_at = |seq: u16| steps.iter().find(|step| step.seq == seq);
    let reply = |event: String, at: DateTime<Utc>, text: String| ThreadComment {
        id: comment_id(run.id, &event, at),
        thread_id: Some(first_comment(run).id),
        text: factory_signed(&text),
    };

    match update {
        RunUpdate::Started => Some(first_comment(run)),
        RunUpdate::StepEnded { seq } => {
            let step = step_at(*seq)?;
            let end = step.end.as_ref()?;
            let mut text = format!(
                "Step {seq}, `{}`{}, ended `{}`.",
                step.entry,
                worked_by(step),
                end.outcome
            );
            // A pull request Step's output starts by naming its pull
            // request; any other Step's output stays on the Run.
            if PULL_REQUEST_OUTCOMES.contains(&end.outcome.as_str())
                && let Some(named) = end.output.lines().next()
            {
                text.push_str(&format!("\n\n{named}"));
            }
            Some(reply(format!("step:{seq}"), end.ended_at, text))
        }
        RunUpdate::GateWaiting { seq } => {
            let step = step_at(*seq)?;
            let gate = step.gate.as_ref()?;
            let question = quoted(&gate.question);
            let text = match gate.kind {
                GateKind::Question => {
                    let role = step
                        .role
                        .as_ref()
                        .map_or_else(|| "agent".to_string(), ToString::to_string);
                    format!(
                        "Step {seq}: the {role} asks the run's owner a question.\n\n> {question}"
                    )
                }
                GateKind::Flow => {
                    let answers: Vec<String> = gate
                        .answers
                        .iter()
                        .map(|answer| format!("`{answer}`"))
                        .collect();
                    format!(
                        "Step {seq} waits for the run's owner to answer {}.\n\n> {question}",
                        answers.join(" or ")
                    )
                }
            };
            Some(reply(format!("waiting:{seq}"), gate.opened_at, text))
        }
        RunUpdate::GateAnswered { seq } => {
            let gate = step_at(*seq)?.gate.as_ref()?;
            let answer = gate.answer.as_ref()?;
            let text = format!(
                "Step {seq}'s gate was answered `{}` {}.",
                answer.answer,
                answered_by(answer)
            );
            Some(reply(format!("answer:{seq}"), answer.answered_at, text))
        }
        RunUpdate::GateTold { seq, question } => {
            let end = step_at(*seq)?.end.as_ref()?;
            let text = format!(
                "Step {seq} tells the run's owner, and the run goes on.\n\n> {}",
                quoted(question)
            );
            Some(reply(format!("notice:{seq}"), end.ended_at, text))
        }
        RunUpdate::Ended => {
            let end = run.end.as_ref()?;
            let text = format!("**The factory run ended** `{}`.", end.outcome);
            Some(reply("end".to_string(), end.ended_at, text))
        }
    }
}

/// The start of `text`, on one line, as a comment quotes it: an agent may
/// have written it, so it opens no tag, mention or code block.
fn quoted(text: &str) -> String {
    escape(&FactoryGateWaitingMetadata::excerpt(text))
}

/// Who did `step`'s work, as a comment names them after its entry: its
/// Role and Agent, when it had them.
fn worked_by(step: &Step) -> String {
    match (&step.role, &step.agent) {
        (Some(role), Some(agent)) => format!(" ({role}, {agent})"),
        (Some(role), None) => format!(" ({role})"),
        _ => String::new(),
    }
}

/// Who or what gave `answer`, as a comment names them.
fn answered_by(answer: &GateAnswer) -> String {
    if let Some(rule) = answer.rule {
        return format!("by the `{rule}` Standing Rule");
    }
    if answer.by_stand_in {
        return "by the stand-in, for the run's owner".to_string();
    }
    let person = answer.user_id.as_ref().map_or_else(
        || "a person".to_string(),
        |user| user.email_str().to_string(),
    );
    match answer.reply_confidence {
        Some(_) => format!("by {person}, read from their words"),
        None => format!("by {person}"),
    }
}
