use crate::domain::standing_rules::RunRules;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::models::{
    AgentName, EntryId, HandBack, HandedBack, NewRun, NewSteer, Outcome, Role, RunId, RunInsert,
    SteerAction, SteerRefusal, StepKey, StepSession,
};
use crate::domain::ports::RunRepository;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

/// A started `write` Step of a new Run of Julia's, working in a session it
/// claimed offering `done`.
async fn writing(repo: &PgRunRepository) -> (StepKey, StepSession) {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Add dark mode",
            user_id: &julia,
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };
    let key = StepKey { run_id: id, seq: 1 };
    let entry = EntryId::try_from("write".to_string()).expect("entry");
    repo.start_step(key, &entry, 1).await.expect("start");
    let session = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(AgentName::try_from("primary".to_string()).expect("agent")),
        bot: Some(Uuid::now_v7()),
    };
    repo.claim_session(key, session.clone(), &writer(), &[outcome("done")])
        .await
        .expect("claim");
    (key, session)
}

fn writer() -> Role {
    Role::try_from("writer".to_string()).expect("valid role")
}

/// What the Step's work reads of its steering that a test sets.
#[derive(Debug, PartialEq)]
struct Steering {
    paused: bool,
    stop_asked: bool,
    prompt: Option<Uuid>,
}

async fn steering_of(repo: &PgRunRepository, key: StepKey) -> Steering {
    let steering = repo.steering(key).await.expect("read");
    assert!(
        steering.started_at.is_some(),
        "the work reads when the step started"
    );
    Steering {
        paused: steering.paused,
        stop_asked: steering.stop_asked,
        prompt: steering.prompt,
    }
}

fn note(text: &str) -> (NewSteer, Uuid) {
    let prompt_id = Uuid::now_v7();
    (
        NewSteer::Note {
            note: text.to_string(),
            prompt_id,
        },
        prompt_id,
    )
}

/// A note becomes the prompt the Step follows, a pause holds the Step and
/// refuses another note or pause, and a resume ends it and is followed in
/// turn. Each is kept on the Step, in order, and the session they act on is
/// the Step's, with its Role.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn notes_pauses_and_resumes_steer_the_step_in_turn(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, session) = writing(&repo).await;
    let (first, first_prompt) = note("Use the theme tokens");

    let steered = repo
        .steer(key, &first)
        .await
        .expect("steer")
        .expect("a running step takes a note");
    assert_eq!(steered.session, session);
    assert_eq!(steered.role, writer());
    assert_eq!(steered.previous_prompt, None);
    assert_eq!(
        steering_of(&repo, key).await,
        Steering {
            paused: false,
            stop_asked: false,
            prompt: Some(first_prompt),
        }
    );

    repo.steer(key, &NewSteer::Pause)
        .await
        .expect("steer")
        .expect("a running step pauses");
    let (second, _) = note("And the dark palette");
    assert_eq!(
        repo.steer(key, &second).await.expect("steer"),
        Err(SteerRefusal::Paused)
    );
    assert_eq!(
        repo.steer(key, &NewSteer::Pause).await.expect("steer"),
        Err(SteerRefusal::Paused)
    );
    assert!(repo.steering(key).await.expect("read").paused);

    let resume_prompt = Uuid::now_v7();
    let resumed = repo
        .steer(
            key,
            &NewSteer::Resume {
                note: Some("Go on".to_string()),
                prompt_id: resume_prompt,
            },
        )
        .await
        .expect("steer")
        .expect("a paused step resumes");
    assert_eq!(resumed.previous_prompt, Some(first_prompt));
    assert_eq!(
        steering_of(&repo, key).await,
        Steering {
            paused: false,
            stop_asked: false,
            prompt: Some(resume_prompt),
        }
    );
    assert_eq!(
        repo.steer(
            key,
            &NewSteer::Resume {
                note: None,
                prompt_id: Uuid::now_v7(),
            },
        )
        .await
        .expect("steer"),
        Err(SteerRefusal::NotPaused)
    );

    let steps = repo.list_steps(key.run_id).await.expect("list");
    let terminal = &steps[0].terminal;
    assert_eq!(terminal.session_id, Some(session.session_id));
    assert_eq!(terminal.paused_at, None);
    let kept: Vec<(SteerAction, Option<&str>)> = terminal
        .steers
        .iter()
        .map(|steer| (steer.action, steer.note.as_deref()))
        .collect();
    assert_eq!(
        kept,
        [
            (SteerAction::Note, Some("Use the theme tokens")),
            (SteerAction::Pause, None),
            (SteerAction::Resume, Some("Go on")),
        ]
    );
}

/// Once the Step's work closes its steering, with nothing more to follow,
/// the owner can't steer it until the factory's next prompt opens it again,
/// which the Step then follows rather than the owner's note. A close that
/// does not name the note the owner sent since is refused, so the work
/// reads it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn closed_steering_opens_again_with_the_factorys_next_prompt(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, session) = writing(&repo).await;
    let (sent, prompt) = note("Check the contrast");
    repo.steer(key, &sent)
        .await
        .expect("steer")
        .expect("a note");

    assert!(
        !repo.close_steering(key, None).await.expect("close"),
        "a note the work never read keeps the steering open"
    );
    assert!(repo.close_steering(key, Some(prompt)).await.expect("close"));
    let (late, _) = note("One more thing");
    assert_eq!(
        repo.steer(key, &late).await.expect("steer"),
        Err(SteerRefusal::NotWorking)
    );

    let checks = Uuid::now_v7();
    assert!(
        repo.begin_turn(key, session.session_id, checks, 2, &[outcome("passed")])
            .await
            .expect("turn")
    );
    assert_eq!(
        repo.steering(key).await.expect("read").prompt,
        None,
        "the step follows the factory's prompt"
    );
    repo.steer(key, &late)
        .await
        .expect("steer")
        .expect("the agent works on a turn again");
}

/// A note leaves the agent's hand back as it was, so the turn it steered
/// still ends with it; a stop refuses everything after it; and a Step that
/// ended takes nothing.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_stop_ends_the_steering_and_a_note_keeps_the_hand_back(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, session) = writing(&repo).await;
    let done = HandBack {
        outcome: outcome("done"),
        output: "Wrote the toggle".to_string(),
    };
    repo.hand_back(session.session_id, &done)
        .await
        .expect("hand back");
    let (sent, _) = note("Name it Dark");
    repo.steer(key, &sent).await.expect("steer").expect("note");
    assert_eq!(
        repo.handed_back(key, session.action_id)
            .await
            .expect("read"),
        HandedBack::Handed(done)
    );

    repo.steer(key, &NewSteer::Stop)
        .await
        .expect("steer")
        .expect("a running step stops");
    assert!(repo.steering(key).await.expect("read").stop_asked);
    for steer in [NewSteer::Pause, NewSteer::Stop] {
        assert_eq!(
            repo.steer(key, &steer).await.expect("steer"),
            Err(SteerRefusal::Stopping)
        );
    }
    assert!(
        !repo.close_steering(key, None).await.expect("close"),
        "a stop asked keeps the work reading it"
    );

    repo.end_step(key, &outcome("failed"), "stopped")
        .await
        .expect("end");
    assert_eq!(
        repo.steer(key, &NewSteer::Pause).await.expect("steer"),
        Err(SteerRefusal::Ended)
    );
    assert_eq!(
        repo.list_steps(key.run_id).await.expect("list")[0]
            .terminal
            .session_id,
        None,
        "an ended step has no session to watch"
    );
}

/// A note the session refused is kept undelivered and no longer followed;
/// a refused resume pauses the Step again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_refused_prompt_is_undelivered_and_no_longer_followed(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, _) = writing(&repo).await;
    let (first, first_prompt) = note("First");
    repo.steer(key, &first).await.expect("steer").expect("note");
    let (second, second_prompt) = note("Second");
    let steered = repo
        .steer(key, &second)
        .await
        .expect("steer")
        .expect("note");

    repo.unsteer(key, second_prompt, steered.previous_prompt)
        .await
        .expect("unsteer");
    assert_eq!(
        repo.steering(key).await.expect("read").prompt,
        Some(first_prompt)
    );

    repo.steer(key, &NewSteer::Pause)
        .await
        .expect("steer")
        .expect("pause");
    let resume_prompt = Uuid::now_v7();
    repo.steer(
        key,
        &NewSteer::Resume {
            note: None,
            prompt_id: resume_prompt,
        },
    )
    .await
    .expect("steer")
    .expect("resume");
    repo.unsteer(key, resume_prompt, Some(first_prompt))
        .await
        .expect("unsteer");
    assert_eq!(
        steering_of(&repo, key).await,
        Steering {
            paused: true,
            stop_asked: false,
            prompt: Some(first_prompt),
        }
    );

    let undelivered: Vec<bool> = repo.list_steps(key.run_id).await.expect("list")[0]
        .terminal
        .steers
        .iter()
        .map(|steer| steer.undelivered)
        .collect();
    assert_eq!(undelivered, [false, true, false, true]);
}

/// A Step no agent works on in a session of its own takes no steering.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_step_without_a_session_takes_no_steering(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, _) = writing(&repo).await;
    let other = StepKey {
        run_id: key.run_id,
        seq: 2,
    };
    let entry = EntryId::try_from("publish".to_string()).expect("entry");
    repo.start_step(other, &entry, 1).await.expect("start");
    assert_eq!(
        repo.steer(other, &NewSteer::Pause).await.expect("steer"),
        Err(SteerRefusal::NoSession)
    );
}

/// A Step handed to another Agent's session follows none of the owner's
/// prompts to the session it left, and takes their steering again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_step_handed_to_a_new_session_follows_none_of_the_old_one(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, session) = writing(&repo).await;
    let (sent, prompt) = note("Use the tokens");
    repo.steer(key, &sent).await.expect("steer").expect("note");
    assert!(repo.close_steering(key, Some(prompt)).await.expect("close"));

    let backup = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(AgentName::try_from("backup".to_string()).expect("agent")),
        bot: Some(Uuid::now_v7()),
    };
    let passed = repo
        .pass_session(
            key,
            session.session_id,
            &writer(),
            &crate::domain::models::PassedOver {
                agent: AgentName::try_from("primary".to_string()).expect("agent"),
                reason: "ai_allowance_exhausted".to_string(),
            },
            Some(&backup),
        )
        .await
        .expect("pass");
    assert!(passed);

    assert_eq!(steering_of(&repo, key).await.prompt, None);
    repo.steer(key, &NewSteer::Pause)
        .await
        .expect("steer")
        .expect("the new session's agent takes steering");
}

/// The Step that goes on after its agent's question works in the asking
/// Step's session, and takes the owner's steering though the asking Step's
/// closed, following none of the prompts the owner sent the asking Step.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn the_step_going_on_from_a_question_takes_steering(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (asking, session) = writing(&repo).await;
    let (sent, prompt) = note("Before you ask");
    repo.steer(asking, &sent)
        .await
        .expect("steer")
        .expect("note");
    assert!(
        repo.close_steering(asking, Some(prompt))
            .await
            .expect("close")
    );
    repo.end_step(asking, &outcome("question"), "Which palette?")
        .await
        .expect("end");

    let going_on = StepKey {
        run_id: asking.run_id,
        seq: 2,
    };
    let entry = EntryId::try_from("write".to_string()).expect("entry");
    repo.start_step(going_on, &entry, 1).await.expect("start");
    repo.claim_session(
        going_on,
        StepSession {
            action_id: Uuid::now_v7(),
            ..session.clone()
        },
        &writer(),
        &[outcome("done")],
    )
    .await
    .expect("claim");

    assert_eq!(steering_of(&repo, going_on).await.prompt, None);
    let (later, _) = note("Use the dark one");
    let steered_now = repo
        .steer(going_on, &later)
        .await
        .expect("steer")
        .expect("the step going on takes a note");
    assert_eq!(steered_now.session.session_id, session.session_id);
    assert_eq!(
        repo.list_steps(asking.run_id).await.expect("list")[1]
            .terminal
            .session_id,
        Some(session.session_id)
    );
}

/// A Step whose work ended it because it stayed paused says so, and only a
/// paused Step can; a Step between its agent's turns says that too.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_paused_step_that_ran_out_of_time_says_so(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (key, _) = writing(&repo).await;
    repo.pause_ran_out(key).await.expect("not paused");
    assert!(!repo.steering(key).await.expect("read").pause_ran_out);
    assert!(repo.close_steering(key, None).await.expect("close"));
    assert!(
        repo.list_steps(key.run_id).await.expect("list")[0]
            .terminal
            .between_turns
    );

    let checks = Uuid::now_v7();
    let session = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");
    repo.begin_turn(key, session.session_id, checks, 2, &[outcome("passed")])
        .await
        .expect("turn");
    repo.steer(key, &NewSteer::Pause)
        .await
        .expect("steer")
        .expect("pause");
    repo.pause_ran_out(key).await.expect("ran out");
    assert!(repo.steering(key).await.expect("read").pause_ran_out);
    let terminal = &repo.list_steps(key.run_id).await.expect("list")[0].terminal;
    assert!(terminal.pause_ran_out_at.is_some());
    assert!(!terminal.between_turns);
}
