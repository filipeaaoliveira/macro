use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::PgRunRepository;
use crate::domain::models::{
    ClaimedSession, EntryId, HandBack, Outcome, Run, RunId, RunInsert, RunOutcome, StepKey,
    StepSession, TaskLink,
};
use crate::domain::ports::RunRepository;

const LOGIN_TASK: &str = "00000000-0000-7000-8000-000000000101";
const DARK_MODE_TASK: &str = "00000000-0000-7000-8000-000000000102";

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid entry id")
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

fn task(id: &str) -> Uuid {
    Uuid::parse_str(id).expect("valid task id")
}

fn key(run_id: RunId, seq: u16) -> StepKey {
    StepKey { run_id, seq }
}

/// Keep a new Run of Julia's with no task.
async fn insert(repo: &PgRunRepository, id: RunId, request: &str) -> Run {
    match repo
        .insert_run(id, request, &julia(), None)
        .await
        .expect("insert")
    {
        RunInsert::Inserted(run) => run,
        RunInsert::NoSuchTask => panic!("a run with no task has no task to miss"),
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
        repo.answer_gate(key(id, seq), &outcome("yes"), Some(output), &julia())
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
    )
    .await
    .expect("answer");
    repo.answer_gate(key(id, 1), &outcome("approve"), None, &julia())
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
    repo.answer_gate(key(id, 1), &outcome("approve"), None, &gone)
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

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "tasks"))
)]
async fn a_run_is_kept_with_its_task_in_one_write(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();

    let inserted = repo
        .insert_run(id, "Fix the login bug", &julia(), Some(task(LOGIN_TASK)))
        .await
        .expect("insert");
    let missing = repo
        .insert_run(
            RunId::generate(),
            "Fix the login bug",
            &julia(),
            Some(Uuid::now_v7()),
        )
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

fn session() -> StepSession {
    StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
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
        .claim_session(key(id, 1), first, &offered)
        .await
        .expect("claim");
    let retried = repo
        .claim_session(key(id, 1), session(), &offered)
        .await
        .expect("claim again");

    assert_eq!(
        claimed,
        ClaimedSession {
            session: first,
            new: true
        }
    );
    assert_eq!(
        retried,
        ClaimedSession {
            session: first,
            new: false
        }
    );
    assert_eq!(
        repo.claimed_session(key(id, 1)).await.expect("read"),
        Some(first)
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
            &[outcome("done"), outcome("unchanged")],
        )
        .await
        .expect("claim");
    let session_id = claimed.session.session_id;
    let unchanged = HandBack {
        outcome: outcome("unchanged"),
        output: "Nothing to do".to_string(),
    };
    let committed = HandBack {
        outcome: outcome("done"),
        output: "Committed the toggle".to_string(),
    };

    assert_eq!(repo.handed_back(key(id, 1)).await.expect("read"), None);
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
        repo.handed_back(key(id, 1)).await.expect("read"),
        Some(committed)
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
        repo.claim_session(step, session(), &[outcome("done")])
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

    let holds = async |step| repo.holds_session(step).await.expect("read");
    assert!(!holds(key(id, 1)).await, "let go");
    assert!(!holds(key(id, 2)).await, "ended");
    assert!(holds(key(id, 3)).await, "still working");
    assert!(!holds(key(ended_run, 1)).await, "its run ended");
    assert!(!holds(key(id, 4)).await, "never claimed one");
}
