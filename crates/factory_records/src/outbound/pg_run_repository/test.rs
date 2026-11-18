use crate::domain::standing_rules::RunRules;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::PgRunRepository;
use crate::domain::models::{
    AgentName, ClaimedSession, EntryId, GateKind, HandBack, HandedBack, NewRun, Outcome,
    PassedOver, Repository, Role, Run, RunId, RunInsert, RunOutcome, StepKey, StepSession,
    TaskLink,
};
use crate::domain::ports::RunRepository;

mod control_files;
mod definition;
mod waiting;

const LOGIN_TASK: &str = "00000000-0000-7000-8000-000000000101";
const DARK_MODE_TASK: &str = "00000000-0000-7000-8000-000000000102";

pub(super) fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid entry id")
}

pub(super) fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

pub(super) fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

fn task(id: &str) -> Uuid {
    Uuid::parse_str(id).expect("valid task id")
}

pub(super) fn key(run_id: RunId, seq: u16) -> StepKey {
    StepKey { run_id, seq }
}

/// Keep a new Run of Julia's with no task.
pub(super) async fn insert(repo: &PgRunRepository, id: RunId, request: &str) -> Run {
    match repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request,
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    {
        RunInsert::Inserted(run) => *run,
        RunInsert::NoSuchTask
        | RunInsert::TaskBusy
        | RunInsert::FiringStarted
        | RunInsert::QueueFull(_) => {
            panic!("a run with no task has no task to miss")
        }
    }
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_new_run_is_running_with_no_steps(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();

    let inserted = insert(&repo, id, "Summarise the release notes").await;
    let read = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run exists");

    assert_eq!(read, inserted);
    assert_eq!(read.user_id, julia());
    assert_eq!(read.end, None);
    assert!(repo.list_steps(id).await.expect("steps").is_empty());
}

/// A reset numbers an entry's visits from 1 again: `ci-fix` resets
/// `review`, whose second round is its visit 1 once more. Both rounds keep
/// their own Step and Gate.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_reset_entry_keeps_every_visit_it_made(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Fix the flaky test").await;
    let rounds = [
        (1, "review", "Two blockers"),
        (2, "ci-fix", "Fixed the build"),
        (3, "review", "Clean"),
    ];

    for (seq, name, output) in rounds {
        repo.start_step(key(id, seq), &entry(name), 1)
            .await
            .expect("start");
        repo.open_gate(key(id, seq), "Go on?", &[outcome("yes")])
            .await
            .expect("open");
        repo.answer_gate(
            key(id, seq),
            &outcome("yes"),
            Some(output),
            &julia(),
            None,
            None,
        )
        .await
        .expect("answer");
        repo.end_step(key(id, seq), &outcome("done"), output)
            .await
            .expect("end");
    }

    let steps = repo.list_steps(id).await.expect("steps");
    let kept: Vec<(u16, &str, u16, &str)> = steps
        .iter()
        .map(|step| {
            let end = step.end.as_ref().expect("the step ended");
            (
                step.seq,
                step.entry.as_str(),
                step.visit,
                end.output.as_str(),
            )
        })
        .collect();
    assert_eq!(
        kept,
        [
            (1, "review", 1, "Two blockers"),
            (2, "ci-fix", 1, "Fixed the build"),
            (3, "review", 1, "Clean"),
        ]
    );
    let notes: Vec<Option<&str>> = steps
        .iter()
        .map(|step| {
            step.gate
                .as_ref()
                .and_then(|gate| gate.answer.as_ref())
                .and_then(|answer| answer.note.as_deref())
        })
        .collect();
    assert_eq!(
        notes,
        [Some("Two blockers"), Some("Fixed the build"), Some("Clean")]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_retried_step_keeps_one_step_and_its_first_end(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Reply to the request").await;

    repo.start_step(key(id, 1), &entry("reply"), 1)
        .await
        .expect("start");
    repo.start_step(key(id, 1), &entry("reply"), 1)
        .await
        .expect("start again");
    repo.end_step(key(id, 1), &outcome("done"), "Replied")
        .await
        .expect("end");
    repo.end_step(key(id, 1), &outcome("failed"), "Too late")
        .await
        .expect("end again");

    let steps = repo.list_steps(id).await.expect("steps");
    assert_eq!(steps.len(), 1);
    let end = steps[0].end.as_ref().expect("the step ended");
    assert_eq!(
        (end.outcome.as_str(), end.output.as_str()),
        ("done", "Replied")
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_run_that_ended_keeps_its_first_end(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Reply to the request").await;

    repo.end_run(id, RunOutcome::Success).await.expect("end");
    repo.end_run(id, RunOutcome::Failed)
        .await
        .expect("end again");

    let run = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run exists");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Success));
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn cancelling_a_run_ends_its_open_steps_and_nothing_ends_them_later(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the migration").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");

    repo.end_run(id, RunOutcome::Cancelled).await.expect("end");
    repo.end_step(key(id, 1), &outcome("done"), "The plan, finished late")
        .await
        .expect("a late end");

    let run = repo.get_run(id).await.expect("get").expect("exists");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Cancelled));
    let steps = repo.list_steps(id).await.expect("steps");
    let end = steps[0].end.as_ref().expect("the step ended");
    assert_eq!(end.outcome.as_str(), "cancelled");
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn deleting_a_run_removes_its_steps(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    insert(&repo, id, "Reply to the request").await;
    repo.start_step(key(id, 1), &entry("reply"), 1)
        .await
        .expect("start");

    repo.delete_run(id).await.expect("delete");

    assert_eq!(repo.get_run(id).await.expect("get"), None);
    let steps = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM factory_step WHERE run_id = $1"#,
        id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(steps, 0);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_gate_waits_then_keeps_its_first_answer(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the migration").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let answers = [outcome("approve"), outcome("revise")];

    repo.open_gate(key(id, 1), "Approve the plan?", &answers)
        .await
        .expect("open");
    repo.open_gate(key(id, 1), "Approve the plan?", &answers)
        .await
        .expect("open again");
    let waiting = repo.list_steps(id).await.expect("steps");
    let gate = waiting[0].gate.as_ref().expect("the step has a gate");
    assert_eq!(gate.question, "Approve the plan?");
    assert_eq!(gate.answers, answers);
    assert_eq!(gate.answer, None);

    repo.answer_gate(
        key(id, 1),
        &outcome("revise"),
        Some("Keep the old index"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("answer");
    repo.answer_gate(key(id, 1), &outcome("approve"), None, &julia(), None, None)
        .await
        .expect("answer again");

    let answered = repo.list_steps(id).await.expect("steps");
    let answer = answered[0]
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref())
        .expect("the gate is answered");
    assert_eq!(answer.answer, outcome("revise"));
    assert_eq!(answer.note.as_deref(), Some("Keep the old index"));
    assert_eq!(answer.user_id, Some(julia()));
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn an_answer_from_an_account_gone_since_records_no_one(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the migration").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.open_gate(key(id, 1), "Approve the plan?", &[outcome("approve")])
        .await
        .expect("open");

    // The API refuses an unknown user, but an account can go between that
    // check and the workflow recording the answer.
    let gone = MacroUserIdStr::try_from("macro|gone@example.com".to_string()).expect("valid");
    repo.answer_gate(key(id, 1), &outcome("approve"), None, &gone, None, None)
        .await
        .expect("answer");

    let steps = repo.list_steps(id).await.expect("steps");
    let answer = steps[0]
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref())
        .expect("the gate is answered");
    assert_eq!(answer.answer, outcome("approve"));
    assert_eq!(answer.user_id, None);
}

/// A Run kept with a repository reads it back, and keeps the last commit it
/// asks a person to merge; a Run with none reads none of either.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_run_keeps_its_repository_and_the_commit_it_asks_to_merge(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };

    let inserted = repo
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Collapse dashes",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: Some(&repository),
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert");
    repo.set_merge_head(id, "1111111111111111111111111111111111111111")
        .await
        .expect("set");
    repo.set_merge_head(id, "2222222222222222222222222222222222222222")
        .await
        .expect("set again");
    let without = insert(&repo, RunId::generate(), "Summarise").await;

    let RunInsert::Inserted(run) = inserted else {
        panic!("a run with no task has no task to miss");
    };
    assert_eq!(run.repository.as_ref(), Some(&repository));
    let read = repo.get_run(id).await.expect("get").expect("kept");
    assert_eq!(read.repository, Some(repository));
    assert_eq!(
        repo.merge_head(id).await.expect("read").as_deref(),
        Some("2222222222222222222222222222222222222222")
    );
    assert_eq!(without.repository, None);
    assert_eq!(repo.merge_head(without.id).await.expect("read"), None);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "tasks"))
)]
async fn a_run_is_kept_with_its_task_in_one_write(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();

    let inserted = repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request: "Fix the login bug",
            user_id: &julia(),
            task_id: Some(task(LOGIN_TASK)),
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert");
    let missing = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Fix the login bug",
            user_id: &julia(),
            task_id: Some(Uuid::now_v7()),
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert for a missing task");

    let RunInsert::Inserted(run) = inserted else {
        panic!("the task exists");
    };
    assert_eq!(run.task_id, Some(task(LOGIN_TASK)));
    assert_eq!(missing, RunInsert::NoSuchTask);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "tasks"))
)]
async fn a_run_links_to_one_task_at_a_time(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Fix the login bug").await;

    let first = repo.link_task(id, task(LOGIN_TASK)).await.expect("link");
    let second = repo
        .link_task(id, task(DARK_MODE_TASK))
        .await
        .expect("relink");

    assert_eq!((first, second), (TaskLink::Linked, TaskLink::Linked));
    let run = repo.get_run(id).await.expect("get").expect("exists");
    assert_eq!(run.task_id, Some(task(DARK_MODE_TASK)));
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_task_that_does_not_exist_or_a_run_that_does_not_is_not_linked(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Fix the login bug").await;

    let no_task = repo.link_task(id, task(LOGIN_TASK)).await.expect("link");
    let no_run = repo
        .link_task(RunId::generate(), task(LOGIN_TASK))
        .await
        .expect("link");

    assert_eq!(no_task, TaskLink::NoSuchTask);
    assert_eq!(no_run, TaskLink::NoSuchRun);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "tasks"))
)]
async fn deleting_its_task_unlinks_a_run_and_keeps_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    insert(&repo, id, "Fix the login bug").await;
    repo.link_task(id, task(LOGIN_TASK)).await.expect("link");

    sqlx::query!(r#"DELETE FROM "Document" WHERE id = $1"#, LOGIN_TASK)
        .execute(&pool)
        .await
        .expect("delete the task");

    let run = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run is kept");
    assert_eq!(run.task_id, None);
}

/// The session the Step `step` works in.
async fn claimed_id(repo: &PgRunRepository, step: StepKey) -> Uuid {
    repo.claimed_session(step)
        .await
        .expect("read")
        .expect("claimed")
        .session_id
}

/// The pass of `name`'s Agent for lack of quota.
fn passed(name: &str) -> PassedOver {
    PassedOver {
        agent: agent(name),
        reason: "ai_allowance_exhausted".to_string(),
    }
}

fn agent(name: &str) -> AgentName {
    AgentName::try_from(name.to_string()).expect("valid agent")
}

fn planner() -> Role {
    Role::try_from("planner".to_string()).expect("valid role")
}

/// A new session of the `primary` Agent.
fn session() -> StepSession {
    session_of("primary")
}

fn session_of(name: &str) -> StepSession {
    StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(agent(name)),
        bot: Some(Uuid::now_v7()),
    }
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_retried_step_keeps_the_session_its_first_attempt_claimed(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the migration").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let offered = [outcome("done")];
    let first = session();

    let claimed = repo
        .claim_session(key(id, 1), first.clone(), &planner(), &offered)
        .await
        .expect("claim");
    let retried = repo
        .claim_session(key(id, 1), session(), &planner(), &offered)
        .await
        .expect("claim again");

    assert_eq!(
        claimed,
        ClaimedSession {
            session: first.clone(),
            new: true
        }
    );
    assert_eq!(
        retried,
        ClaimedSession {
            session: first.clone(),
            new: false
        }
    );
    assert_eq!(
        repo.claimed_session(key(id, 1)).await.expect("read"),
        Some(first.clone())
    );
    let step = repo
        .session_step(first.session_id)
        .await
        .expect("read")
        .expect("the session has a step");
    assert_eq!((step.key, step.owner), (key(id, 1), julia()));
    assert_eq!(step.offered, offered);
    assert!(!step.ended);
}

/// A Step whose Agent could not start goes on in a session of the next
/// Agent, and keeps its Role, the Agent that took it, and the one passed
/// over with why. Only the session the Step works in is handed on, once.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_step_handed_to_the_next_agent_keeps_who_was_passed_over_and_why(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let first = repo
        .claim_session(key(id, 1), session(), &planner(), &[outcome("done")])
        .await
        .expect("claim")
        .session;
    repo.record_prepare(key(id, 1), claimed_id(&repo, key(id, 1)).await)
        .await
        .expect("prepare");
    let next = session_of("backup");

    let handed_on = repo
        .pass_session(
            key(id, 1),
            first.session_id,
            &planner(),
            &passed("primary"),
            Some(&next),
        )
        .await
        .expect("pass");
    let handed_on_again = repo
        .pass_session(
            key(id, 1),
            first.session_id,
            &planner(),
            &passed("primary"),
            Some(&session_of("spare")),
        )
        .await
        .expect("pass again");

    assert!(handed_on);
    assert!(
        !handed_on_again,
        "the step no longer works in the first session"
    );
    assert_eq!(
        repo.claimed_session(key(id, 1)).await.expect("read"),
        Some(next.clone())
    );
    let stopping = repo
        .stopping_session(key(id, 1), std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    assert!(
        !stopping.opened && !stopping.may_still_open,
        "the next session was not asked to open yet"
    );
    let step = repo
        .session_step(next.session_id)
        .await
        .expect("read")
        .expect("the next session has the step");
    assert_eq!(step.offered, [outcome("done")]);
    let steps = repo.list_steps(id).await.expect("list");
    assert_eq!(steps[0].role, Some(planner()));
    assert_eq!(steps[0].agent, Some(agent("backup")));
    assert_eq!(steps[0].passed_over, [passed("primary")]);
    assert!(
        repo.record_prepare(key(id, 1), first.session_id)
            .await
            .expect("prepare")
            .is_none(),
        "the session handed on is never asked to open again"
    );
    assert!(
        !repo
            .holds_session(key(id, 1), first.session_id)
            .await
            .expect("read")
    );
    assert!(
        repo.holds_session(key(id, 1), next.session_id)
            .await
            .expect("read")
    );
    repo.record_open(key(id, 1), first.session_id)
        .await
        .expect("open");
    let stopping = repo
        .stopping_session(key(id, 1), std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    assert!(
        !stopping.opened,
        "the session handed on is never recorded open"
    );
    assert_eq!(
        stopping.session.bot, next.bot,
        "the next session's bot is kept"
    );
}

/// When the last Agent of the Role cannot start either, the Step keeps its
/// session and records that Agent as passed over too.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn the_last_agent_passed_over_is_recorded_with_the_step_s_own_session(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let last = repo
        .claim_session(
            key(id, 1),
            session_of("backup"),
            &planner(),
            &[outcome("done")],
        )
        .await
        .expect("claim")
        .session;

    let recorded = repo
        .pass_session(
            key(id, 1),
            last.session_id,
            &planner(),
            &passed("backup"),
            None,
        )
        .await
        .expect("pass");
    let recorded_again = repo
        .pass_session(
            key(id, 1),
            last.session_id,
            &planner(),
            &passed("backup"),
            None,
        )
        .await
        .expect("pass again");

    assert!(recorded);
    assert!(!recorded_again, "an overlapping attempt records it once");
    assert_eq!(
        repo.claimed_session(key(id, 1)).await.expect("read"),
        Some(last)
    );
    let steps = repo.list_steps(id).await.expect("list");
    assert_eq!(steps[0].agent, Some(agent("backup")));
    assert_eq!(steps[0].passed_over, [passed("backup")]);
}

/// A Step that claimed its session before Steps recorded their Role and
/// Agent reads with no Agent, so a retry, a stop or a release finds it
/// still; handing it on records the Role and the Agent passed over.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_session_claimed_before_agents_were_recorded_still_reads(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let claimed = repo
        .claim_session(key(id, 1), session(), &planner(), &[outcome("done")])
        .await
        .expect("claim")
        .session;
    // As the step was kept before the migration that added the columns.
    sqlx::query!("UPDATE factory_step SET role = NULL, agent = NULL, bot_id = NULL")
        .execute(&pool)
        .await
        .expect("forget the agent");

    let retried = repo
        .claim_session(key(id, 1), session(), &planner(), &[outcome("done")])
        .await
        .expect("claim again")
        .session;
    let stopping = repo
        .stopping_session(key(id, 1), std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    let next = session_of("backup");
    let handed_on = repo
        .pass_session(
            key(id, 1),
            claimed.session_id,
            &planner(),
            &passed("primary"),
            Some(&next),
        )
        .await
        .expect("pass");

    assert_eq!(retried.session_id, claimed.session_id);
    assert_eq!((retried.agent, retried.bot), (None, None));
    assert_eq!(stopping.session.agent, None);
    assert!(handed_on);
    let steps = repo.list_steps(id).await.expect("list");
    assert_eq!(steps[0].role, Some(planner()));
    assert_eq!(steps[0].agent, Some(agent("backup")));
    assert_eq!(steps[0].passed_over, [passed("primary")]);
}

/// A Step that let go of its session, or ended, is handed to no other
/// Agent: a stop may be deleting the session it works in.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_step_that_let_go_of_its_session_is_handed_to_no_one(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    let mut claimed = Vec::new();
    for seq in [1, 2] {
        repo.start_step(key(id, seq), &entry("plan"), 1)
            .await
            .expect("start");
        let session = repo
            .claim_session(key(id, seq), session(), &planner(), &[outcome("done")])
            .await
            .expect("claim")
            .session;
        claimed.push(session);
    }
    repo.release_session(key(id, 1)).await.expect("release");
    repo.end_step(key(id, 2), &outcome("failed"), "No agent")
        .await
        .expect("end");

    for (seq, from) in [1, 2].into_iter().zip(&claimed) {
        let handed_on = repo
            .pass_session(
                key(id, seq),
                from.session_id,
                &planner(),
                &passed("primary"),
                Some(&session_of("backup")),
            )
            .await
            .expect("pass");
        assert!(!handed_on, "step {seq}");
        assert_eq!(
            repo.claimed_session(key(id, seq))
                .await
                .expect("read")
                .as_ref(),
            Some(from)
        );
    }
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn the_latest_hand_back_counts_until_the_step_ends(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Write the change").await;
    repo.start_step(key(id, 1), &entry("write"), 1)
        .await
        .expect("start");
    let claimed = repo
        .claim_session(
            key(id, 1),
            session(),
            &planner(),
            &[outcome("done"), outcome("unchanged")],
        )
        .await
        .expect("claim");
    let session_id = claimed.session.session_id;
    let first_prompt = claimed.session.action_id;
    let unchanged = HandBack {
        outcome: outcome("unchanged"),
        output: "Nothing to do".to_string(),
    };
    let committed = HandBack {
        outcome: outcome("done"),
        output: "Committed the toggle".to_string(),
    };

    assert_eq!(
        repo.handed_back(key(id, 1), first_prompt)
            .await
            .expect("read"),
        HandedBack::Nothing
    );
    let first = repo
        .hand_back(session_id, &unchanged)
        .await
        .expect("hand back");
    let second = repo
        .hand_back(session_id, &committed)
        .await
        .expect("hand back again");
    repo.end_step(key(id, 1), &outcome("done"), "Committed the toggle")
        .await
        .expect("end");
    let late = repo
        .hand_back(session_id, &unchanged)
        .await
        .expect("hand back after the end");

    assert_eq!((first, second, late), (true, true, false));
    assert_eq!(
        repo.handed_back(key(id, 1), first_prompt)
            .await
            .expect("read"),
        HandedBack::Handed(committed)
    );
}

/// A Step holds its session until it lets go of it, ends, or its Run ends:
/// an attempt still running after that must not open it again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_step_holds_its_session_until_it_lets_go_or_ends(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    let ended_run = RunId::generate();
    insert(&repo, ended_run, "Plan the login fix").await;
    let steps = [key(id, 1), key(id, 2), key(id, 3), key(ended_run, 1)];
    for step in steps {
        repo.start_step(step, &entry("plan"), 1)
            .await
            .expect("start");
        repo.claim_session(step, session(), &planner(), &[outcome("done")])
            .await
            .expect("claim");
    }
    repo.start_step(key(id, 4), &entry("reply"), 1)
        .await
        .expect("start");

    repo.release_session(key(id, 1)).await.expect("release");
    repo.release_session(key(id, 1))
        .await
        .expect("release again");
    repo.end_step(key(id, 2), &outcome("done"), "The plan")
        .await
        .expect("end");
    repo.end_run(ended_run, RunOutcome::Success)
        .await
        .expect("end run");

    let holds = async |step| {
        let Some(claimed) = repo.claimed_session(step).await.expect("read") else {
            return false;
        };
        repo.holds_session(step, claimed.session_id)
            .await
            .expect("read")
    };
    assert!(!holds(key(id, 1)).await, "let go");
    assert!(!holds(key(id, 2)).await, "ended");
    assert!(holds(key(id, 3)).await, "still working");
    assert!(!holds(key(ended_run, 1)).await, "its run ended");
    assert!(!holds(key(id, 4)).await, "never claimed one");
}

/// A session a Step claimed is a Step's session, whether the Step still
/// works in it or not; any other session is not.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_session_a_step_claimed_is_a_step_session(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let claimed = repo
        .claim_session(key(id, 1), session(), &planner(), &[outcome("done")])
        .await
        .expect("claim")
        .session
        .session_id;
    repo.end_step(key(id, 1), &outcome("done"), "The plan")
        .await
        .expect("end");

    assert!(repo.is_step_session(claimed).await.expect("read"));
    assert!(!repo.is_step_session(Uuid::now_v7()).await.expect("read"));
}

/// A stop tells a session that may still open from one that never will by
/// what the attempts recorded: a prepare sent lately may still land; one
/// that opened never counts as gone; and a Step that let go of its session
/// sends no prepare.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_stop_sees_what_the_attempts_recorded_of_a_session(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let claimed = repo
        .claim_session(key(id, 1), session(), &planner(), &[outcome("done")])
        .await
        .expect("claim")
        .session;
    let unprepared = repo
        .stopping_session(key(id, 1), std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");

    let prepared = repo
        .record_prepare(key(id, 1), claimed_id(&repo, key(id, 1)).await)
        .await
        .expect("prepare")
        .is_some();
    let lately = repo
        .stopping_session(key(id, 1), std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    let long_ago = repo
        .stopping_session(key(id, 1), std::time::Duration::ZERO)
        .await
        .expect("read")
        .expect("claimed");
    repo.record_open(key(id, 1), claimed_id(&repo, key(id, 1)).await)
        .await
        .expect("open");
    let opened = repo
        .stopping_session(key(id, 1), std::time::Duration::ZERO)
        .await
        .expect("read")
        .expect("claimed");
    repo.release_session(key(id, 1)).await.expect("release");
    let after_release = repo
        .record_prepare(key(id, 1), claimed_id(&repo, key(id, 1)).await)
        .await
        .expect("prepare")
        .is_some();

    assert_eq!(unprepared.session, claimed);
    assert!(!unprepared.opened && !unprepared.may_still_open);
    assert!(prepared);
    assert!(lately.may_still_open && !lately.opened);
    assert!(!long_ago.may_still_open);
    assert!(opened.opened);
    assert!(
        !after_release,
        "a Step that let go of its session sends no prepare"
    );
}

/// A prepare that was refused, or never showed its session open, is no
/// longer in flight; one whose session opened stays on record. A Step that
/// ended sends no prepare.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_forgotten_prepare_is_not_in_flight_and_an_ended_step_sends_none(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    for seq in [1, 2, 3] {
        repo.start_step(key(id, seq), &entry("plan"), 1)
            .await
            .expect("start");
        repo.claim_session(key(id, seq), session(), &planner(), &[outcome("done")])
            .await
            .expect("claim");
        repo.record_prepare(key(id, seq), claimed_id(&repo, key(id, seq)).await)
            .await
            .expect("prepare")
            .expect("recorded");
    }
    repo.record_open(key(id, 2), claimed_id(&repo, key(id, 2)).await)
        .await
        .expect("open");
    repo.end_step(key(id, 3), &outcome("done"), "The plan")
        .await
        .expect("end");

    let first = repo
        .record_prepare(key(id, 1), claimed_id(&repo, key(id, 1)).await)
        .await
        .expect("prepare")
        .expect("recorded");
    let second = repo
        .record_prepare(key(id, 2), claimed_id(&repo, key(id, 2)).await)
        .await
        .expect("prepare")
        .expect("recorded");
    repo.forget_prepare(key(id, 1), first)
        .await
        .expect("forget");
    repo.forget_prepare(key(id, 2), second)
        .await
        .expect("forget");
    let ended = repo
        .record_prepare(key(id, 3), claimed_id(&repo, key(id, 3)).await)
        .await
        .expect("prepare");

    let in_flight = async |seq| {
        repo.stopping_session(key(id, seq), std::time::Duration::from_secs(3600))
            .await
            .expect("read")
            .expect("claimed")
            .may_still_open
    };
    assert!(!in_flight(1).await, "refused");
    assert!(in_flight(2).await, "opened: kept");
    assert!(ended.is_none(), "an ended Step sends no prepare");
}

/// An attempt that gives up on its prepare clears its own record only: a
/// later attempt's prepare, still in flight, stays on record.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn an_old_attempt_never_forgets_a_newer_attempt_s_prepare(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the dark mode toggle").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.claim_session(key(id, 1), session(), &planner(), &[outcome("done")])
        .await
        .expect("claim");
    let old = repo
        .record_prepare(key(id, 1), claimed_id(&repo, key(id, 1)).await)
        .await
        .expect("prepare")
        .expect("recorded");
    let newer = repo
        .record_prepare(key(id, 1), claimed_id(&repo, key(id, 1)).await)
        .await
        .expect("prepare")
        .expect("recorded");

    repo.forget_prepare(key(id, 1), old).await.expect("forget");

    let stopping = repo
        .stopping_session(key(id, 1), std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    assert!(newer > old);
    assert!(
        stopping.may_still_open,
        "the newer prepare is still in flight"
    );
}

pub(super) fn teo() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|teo@example.com".to_string()).expect("valid user id")
}

/// Keep a new Run of `user_id`'s on the task `task_id`.
async fn insert_on_task(
    repo: &PgRunRepository,
    user_id: &MacroUserIdStr<'static>,
    task_id: &str,
    request: &str,
) -> RunId {
    let id = RunId::generate();
    match repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request,
            user_id,
            task_id: Some(task(task_id)),
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    {
        RunInsert::Inserted(run) => run.id,
        RunInsert::NoSuchTask
        | RunInsert::TaskBusy
        | RunInsert::FiringStarted
        | RunInsert::QueueFull(_) => {
            panic!("the task exists")
        }
    }
}

/// A task's Runs are its owner's own, newest first: someone else's Runs on
/// the task, and the owner's Runs on another task or none, are left out.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "tasks"))
)]
async fn a_task_s_runs_are_its_owner_s_newest_first(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let first = insert_on_task(&repo, &julia(), LOGIN_TASK, "Find the bug").await;
    insert_on_task(&repo, &teo(), LOGIN_TASK, "Reproduce the bug").await;
    insert_on_task(&repo, &julia(), DARK_MODE_TASK, "Add the toggle").await;
    insert(&repo, RunId::generate(), "Summarise the notes").await;
    let second = insert_on_task(&repo, &julia(), LOGIN_TASK, "Fix the bug").await;
    let third = insert_on_task(&repo, &julia(), LOGIN_TASK, "Test the fix").await;

    let all = repo
        .list_task_runs(task(LOGIN_TASK), &julia(), 10)
        .await
        .expect("list");
    let newest_two = repo
        .list_task_runs(task(LOGIN_TASK), &julia(), 2)
        .await
        .expect("list");

    let ids: Vec<RunId> = all.iter().map(|run| run.id).collect();
    assert_eq!(ids, [third, second, first]);
    assert!(all.iter().all(|run| run.user_id == julia()));
    let ids: Vec<RunId> = newest_two.iter().map(|run| run.id).collect();
    assert_eq!(ids, [third, second]);
}

/// Each Run's Steps come back in order, under that Run, with their Gates;
/// a Run with no Steps has no entry.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn the_steps_of_several_runs_come_back_by_run(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let planned = RunId::generate();
    let replied = RunId::generate();
    let waiting = RunId::generate();
    for (id, request) in [(planned, "Plan"), (replied, "Reply"), (waiting, "Wait")] {
        insert(&repo, id, request).await;
    }
    repo.start_step(key(planned, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.end_step(key(planned, 1), &outcome("done"), "The plan")
        .await
        .expect("end");
    repo.open_gate(key(planned, 1), "Approve?", &[outcome("approve")])
        .await
        .expect("open");
    repo.start_step(key(planned, 2), &entry("write"), 1)
        .await
        .expect("start");
    repo.start_step(key(replied, 1), &entry("reply"), 1)
        .await
        .expect("start");

    let steps = repo
        .list_runs_steps(&[planned, replied, waiting])
        .await
        .expect("list");

    let places = |id: RunId| -> Vec<u16> { steps[&id].iter().map(|step| step.seq).collect() };
    assert_eq!(places(planned), [1, 2]);
    assert_eq!(places(replied), [1]);
    assert!(!steps.contains_key(&waiting));
    let gate = steps[&planned][0].gate.as_ref().expect("the plan's gate");
    assert_eq!(gate.question, "Approve?");
    assert_eq!(
        steps[&planned],
        repo.list_steps(planned).await.expect("steps")
    );
}

/// The Step that goes on after its agent's question works in the asking
/// Step's session once that Step ended, never while it runs; the session's
/// hand back reaches the running Step, and the asking Step's Gate reads as
/// a question.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_step_going_on_after_a_question_works_in_the_asking_step_s_session(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the export").await;
    let offered = [outcome("done"), Outcome::question()];
    for seq in [1, 2] {
        repo.start_step(key(id, seq), &entry("plan"), 1)
            .await
            .expect("start");
    }
    let asking = repo
        .claim_session(key(id, 1), session(), &planner(), &offered)
        .await
        .expect("claim")
        .session;
    let going_on = StepSession {
        action_id: Uuid::now_v7(),
        ..asking.clone()
    };

    // While the asking Step runs, no other Step takes its session.
    repo.claim_session(key(id, 2), going_on.clone(), &planner(), &offered)
        .await
        .expect_err("a running step's session is its own");

    repo.end_step(key(id, 1), &Outcome::question(), "CSV or JSON?")
        .await
        .expect("end");
    repo.open_gate(key(id, 1), "CSV or JSON?", &[Outcome::answer()])
        .await
        .expect("open");
    let claimed = repo
        .claim_session(key(id, 2), going_on.clone(), &planner(), &offered)
        .await
        .expect("claim the asking step's session");
    assert_eq!(claimed.session, going_on);

    let step = repo
        .session_step(asking.session_id)
        .await
        .expect("read")
        .expect("the session has a step");
    assert_eq!(step.key, key(id, 2));
    assert!(!step.ended);
    let hand_back = HandBack {
        outcome: outcome("done"),
        output: "The plan, in CSV".to_string(),
    };
    assert!(
        repo.hand_back(asking.session_id, &hand_back)
            .await
            .expect("hand back")
    );
    assert_eq!(
        repo.handed_back(key(id, 2), going_on.action_id)
            .await
            .expect("read"),
        HandedBack::Handed(hand_back)
    );
    assert!(repo.is_step_session(asking.session_id).await.expect("read"));

    let steps = repo.list_steps(id).await.expect("steps");
    let gate = steps[0].gate.as_ref().expect("the question's gate");
    assert_eq!(gate.kind, GateKind::Question);
    assert!(gate.kind.answered_only_by_a_person());
}

/// A person is named by the parts of their name they set, and has no name
/// with none set; one with no account has none either.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "names"))
)]
async fn a_person_is_named_by_the_name_they_set(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let person = |id: &str| MacroUserIdStr::try_from(id.to_string()).expect("valid");

    assert_eq!(
        repo.display_name(&person("macro|julia@example.com"))
            .await
            .expect("read")
            .as_deref(),
        Some("Julia Ruiz")
    );
    assert_eq!(
        repo.display_name(&person("macro|teo@example.com"))
            .await
            .expect("read"),
        None
    );
    assert_eq!(
        repo.display_name(&person("macro|nobody@example.com"))
            .await
            .expect("read"),
        None
    );
}
