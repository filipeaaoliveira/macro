//! The task thread: a Run's main updates, posted as its owner to one
//! comment thread on its task, when the task thread section of its
//! Orchestrator Definition is on (`task_thread`, off by default).
//!
//! The Run's start is the thread's first comment, which links the Run;
//! each later update is one reply: each Step's end (a pull request Step's
//! names its pull request), each review round as one comment with its
//! reviewers' results, a Gate that waits for a person, a Gate's answer and
//! who or what gave it (the owner by name, a Standing Rule, the stand-in
//! or the Orchestrator), a notify-only Gate's notice, and the Run's end.
//! One thread per Run keeps the task's comments to one discussion a Run.
//!
//! Every comment is signed as the factory's ([`factory_signed`]), so the
//! owner's notes leave it out, and posted so it notifies no one: the
//! factory tells people of what waits on them with its own notifications.
//! Each has an id derived from its Run, its update and when the update
//! happened, so posting it again, as a retried activity does, is refused
//! as a conflict and posts nothing more. Posting never fails or holds a
//! Run: an update that cannot be posted within [`POST_TIMEOUT`] is logged
//! and dropped.
//!
//! Macro takes a comment's own id only within a day of the time in it, so
//! a first comment that was never posted cannot be posted under its first
//! id once its Run is a day old. A reply whose thread is not there looks
//! for the Run's thread on the task first (its first comment links the
//! Run), and only when there is none posts a new first comment, with an id
//! of the current time; the next reply finds that one the same way.

#[cfg(test)]
mod test;

mod comments;

use std::time::Duration;

use chrono::{DateTime, Utc};
use factory_records::domain::models::{Run, RunId, StepKey};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use uuid::Uuid;

use crate::domain::ports::{PauseNotices, TaskThread, ThreadPosted};

pub use comments::{UpdateContext, first_comment, update_comment};

/// The namespace an update's comment id takes its fixed bytes from, by its
/// Run and its update. Fixed forever.
const THREAD_NAMESPACE: Uuid = Uuid::from_u128(0x6661_6374_6f72_792d_7468_7265_6164_7321);

/// How long posting one update may take, its thread's first comment
/// included, before it is dropped: the activity that posts it records the
/// Run first and must not wait on its comments.
pub const POST_TIMEOUT: Duration = Duration::from_secs(15);

/// What happened in a Run that its task's thread is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunUpdate {
    /// The Run started: its first Step started.
    Started,
    /// The Step `seq` ended. A review panel's Steps are told with their
    /// round instead.
    StepEnded {
        /// Its place among the Run's Steps.
        seq: u16,
    },
    /// The review round whose first Step is at `seq` closed.
    RoundClosed {
        /// The place of its first Step.
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
    /// The Step `seq` is paused, and its work's time runs out at
    /// `ends_by`: it ends failed then unless its owner resumes it.
    PauseRunningOut {
        /// The paused Step's place.
        seq: u16,
        /// When its work's time runs out.
        ends_by: DateTime<Utc>,
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
    R: RunRepository + ReviewRoundRepository,
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
    R: RunRepository + ReviewRoundRepository,
    P: TaskThread,
{
    let read = read_update(repo, thread, run_id, update).await;
    let (run, task_id, comment) = match read {
        Ok(Some(read)) => read,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(error = %error, %run_id, ?update, "a run update was not posted: the run did not read");
            return;
        }
    };

    let posted = thread.post(&run.user_id, task_id, &comment).await;
    let posted = match (posted, comment.thread_id) {
        (Ok(ThreadPosted::NoThread), Some(_)) => {
            reply_in_another_thread(thread, &run, task_id, &comment).await
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

/// The Run `run_id`, its task, and the comment that posts `update`: none
/// when the Run has its task thread section off, works on no task, or what
/// the update tells is not recorded.
async fn read_update<R, P>(
    repo: &R,
    thread: &P,
    run_id: RunId,
    update: &RunUpdate,
) -> Result<Option<(Run, Uuid, ThreadComment)>, String>
where
    R: RunRepository + ReviewRoundRepository,
    P: TaskThread,
{
    let Some(run) = repo.get_run(run_id).await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let on = run
        .standing_rules
        .is_some_and(|rules| rules.definition.task_thread);
    let Some(task_id) = run.task_id.filter(|_| on) else {
        return Ok(None);
    };
    let steps = repo.list_steps(run_id).await.map_err(|e| e.to_string())?;
    let rounds = repo
        .review_rounds(run_id)
        .await
        .map_err(|e| e.to_string())?;

    // An answer is told with the name its person shows others.
    let answerer = match update {
        RunUpdate::GateAnswered { seq } => steps
            .iter()
            .find(|step| step.seq == *seq)
            .and_then(|step| step.gate.as_ref())
            .and_then(|gate| gate.answer.as_ref())
            .and_then(|answer| answer.user_id.clone()),
        _ => None,
    };
    let answerer_name = match &answerer {
        Some(user) => repo.display_name(user).await.map_err(|e| e.to_string())?,
        None => None,
    };

    let run_link = thread.run_link(run_id);
    let context = UpdateContext {
        run_link: &run_link,
        rounds: &rounds,
        answerer_name: answerer_name.as_deref(),
    };
    Ok(update_comment(&run, &steps, &context, update).map(|comment| (run, task_id, comment)))
}

/// Post `comment`, whose thread is not there, in the Run's thread on the
/// task: the one its first comment, found by the link to the Run, starts;
/// or, when there is none, a new one, whose first comment has an id of the
/// current time, as Macro takes only a recent one.
async fn reply_in_another_thread<P: TaskThread>(
    thread: &P,
    run: &Run,
    task_id: Uuid,
    comment: &ThreadComment,
) -> Result<ThreadPosted, P::Err> {
    let run_link = thread.run_link(run.id);
    let root = match thread.find_thread(&run.user_id, task_id, &run_link).await? {
        Some(root) => root,
        None => {
            let first = ThreadComment {
                id: comment_id(run.id, "start", Utc::now()),
                ..first_comment(run, &run_link)
            };
            match thread.post(&run.user_id, task_id, &first).await? {
                ThreadPosted::Posted => first.id,
                ThreadPosted::NoThread => return Ok(ThreadPosted::NoThread),
            }
        }
    };
    let reply = ThreadComment {
        thread_id: Some(root),
        ..comment.clone()
    };
    thread.post(&run.user_id, task_id, &reply).await
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

/// [`PauseNotices`] on the Run's task thread, when its section is on: a
/// comment that notifies no one, as every update the thread posts.
#[derive(Debug, Clone)]
pub struct ThreadPauseNotices<R, P> {
    repo: R,
    thread: P,
}

impl<R, P> ThreadPauseNotices<R, P> {
    /// Notices posted to the task threads `thread` keeps, of the Runs
    /// `repo` reads.
    pub fn new(repo: R, thread: P) -> Self {
        Self { repo, thread }
    }
}

impl<R, P> PauseNotices for ThreadPauseNotices<R, P>
where
    R: RunRepository + ReviewRoundRepository,
    P: TaskThread,
{
    fn running_out(
        &self,
        key: StepKey,
        ends_by: DateTime<Utc>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(post_update(
            &self.repo,
            &self.thread,
            key.run_id,
            RunUpdate::PauseRunningOut {
                seq: key.seq,
                ends_by,
            },
        ))
    }
}
