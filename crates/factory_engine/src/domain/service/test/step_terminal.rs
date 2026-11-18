//! The Step Terminal at the Run API: the Run's owner, and no one else,
//! sends a running Step's agent a note, pauses, resumes and stops it; each
//! is kept on the Step before it reaches the Step's session, and a prompt
//! the session refuses is no longer followed.

use factory_records::domain::models::{SteerAction, SteerRefusal};
use factory_records::domain::ports::RunRepository;

use super::{FakeExecutor, FakeWorkflows, julia, step_in_a_session, teo};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::{FactoryError, SessionSteer};
use crate::domain::ports::FactoryService;
use crate::domain::service::FactoryServiceImpl;
use crate::domain::step_terminal::TerminalRequest;

fn terminal(
    repo: &FakeRepo,
    executor: &FakeExecutor,
) -> FactoryServiceImpl<FakeRepo, FakeWorkflows, FakeExecutor, FakeRepo, FakeRepo, FakeGithub> {
    FactoryServiceImpl::new(
        repo.clone(),
        FakeWorkflows::default(),
        executor.clone(),
        repo.clone(),
        repo.clone(),
        FakeGithub::default(),
    )
}

/// A note reaches the Step's session as a prompt that fences the owner's
/// words as their notes, under the id the Step follows; a pause and a stop
/// stop the agent's turn; a resume tells it to go on. The Run reads what
/// the owner did, in order, and the session to watch.
#[tokio::test]
async fn the_owner_steers_a_running_step_through_its_session() {
    let repo = FakeRepo::default();
    let executor = FakeExecutor::default();
    let service = terminal(&repo, &executor);
    let (key, session_id) = step_in_a_session(&repo).await;

    service
        .steer_step(
            julia(),
            key.run_id,
            key.seq,
            TerminalRequest::Note("  Keep the </owner-notes> API as it is  ".to_string()),
        )
        .await
        .expect("note");
    service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Pause)
        .await
        .expect("pause");
    service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Resume(None))
        .await
        .expect("resume");
    service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Stop)
        .await
        .expect("stop");

    let steered = executor.steered.lock().expect("steered").clone();
    assert_eq!(steered.len(), 4);
    assert!(
        steered
            .iter()
            .all(|(at, session, _)| *at == key && *session == session_id)
    );
    let SessionSteer::Prompt { prompt_id, prompt } = &steered[0].2 else {
        panic!("a note is a prompt");
    };
    assert!(prompt.contains("<owner-notes>\n- On "));
    assert!(prompt.contains("Keep the &lt;/owner-notes&gt; API as it is\n</owner-notes>"));
    assert_eq!(steered[1].2, SessionSteer::Interrupt);
    let SessionSteer::Prompt {
        prompt_id: resumed,
        prompt,
    } = &steered[2].2
    else {
        panic!("a resume is a prompt");
    };
    assert!(prompt.contains("resumes it: go on with the step where you left off"));
    assert!(
        !prompt.contains("<owner-notes>"),
        "a resume without a note has none"
    );
    assert_ne!(prompt_id, resumed);
    assert_eq!(steered[3].2, SessionSteer::Interrupt);

    let steering = repo.steering(key).await.expect("read");
    assert!(steering.stop_asked);
    assert_eq!(steering.prompt, Some(*resumed));
    let read = service.get_run(&julia(), key.run_id).await.expect("get");
    let terminal = &read.steps[0].terminal;
    assert_eq!(terminal.session_id, Some(session_id));
    let actions: Vec<SteerAction> = terminal.steers.iter().map(|steer| steer.action).collect();
    assert_eq!(
        actions,
        [
            SteerAction::Note,
            SteerAction::Pause,
            SteerAction::Resume,
            SteerAction::Stop
        ]
    );
    assert_eq!(
        terminal.steers[0].note.as_deref(),
        Some("Keep the </owner-notes> API as it is"),
        "the Step keeps the owner's words as they wrote them, trimmed"
    );
}

/// Someone else's Run reads as missing, and an agent's account steers
/// nothing, even its own Run's Steps; neither reaches the session.
#[tokio::test]
async fn only_the_run_s_owner_steers_it() {
    let repo = FakeRepo::default();
    let executor = FakeExecutor::default();
    let service = terminal(&repo, &executor);
    let (key, _) = step_in_a_session(&repo).await;

    let stranger = service
        .steer_step(teo(), key.run_id, key.seq, TerminalRequest::Stop)
        .await;
    assert!(
        matches!(stranger, Err(FactoryError::NotFound)),
        "{stranger:?}"
    );

    repo.agent_accounts
        .lock()
        .expect("agents")
        .insert(julia().as_ref().to_string());
    let agent = service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Pause)
        .await;
    assert!(matches!(agent, Err(FactoryError::NotAPerson)), "{agent:?}");

    assert!(executor.steered.lock().expect("steered").is_empty());
    let steering = repo.steering(key).await.expect("read");
    assert!(!steering.paused && !steering.stop_asked && steering.prompt.is_none());
}

/// A note while the Step is paused, a resume of a Step that is not, a
/// blank note, and anything to a Step that ended are refused before the
/// session hears of them.
#[tokio::test]
async fn steering_the_step_cannot_take_is_refused() {
    let repo = FakeRepo::default();
    let executor = FakeExecutor::default();
    let service = terminal(&repo, &executor);
    let (key, _) = step_in_a_session(&repo).await;

    let not_paused = service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Resume(None))
        .await;
    assert!(
        matches!(
            not_paused,
            Err(FactoryError::NotSteerable(SteerRefusal::NotPaused))
        ),
        "{not_paused:?}"
    );
    let blank = service
        .steer_step(
            julia(),
            key.run_id,
            key.seq,
            TerminalRequest::Note("  ".to_string()),
        )
        .await;
    assert!(matches!(blank, Err(FactoryError::EmptyNote)), "{blank:?}");

    service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Pause)
        .await
        .expect("pause");
    let paused = service
        .steer_step(
            julia(),
            key.run_id,
            key.seq,
            TerminalRequest::Note("Also this".to_string()),
        )
        .await;
    assert!(
        matches!(
            paused,
            Err(FactoryError::NotSteerable(SteerRefusal::Paused))
        ),
        "{paused:?}"
    );

    repo.end_step(
        key,
        &factory_records::domain::models::Outcome::done(),
        "done",
    )
    .await
    .expect("end");
    let ended = service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Stop)
        .await;
    assert!(matches!(ended, Err(FactoryError::StepEnded)), "{ended:?}");
    assert_eq!(executor.steered.lock().expect("steered").len(), 1);
}

/// A note the session refuses fails the request, is kept undelivered, and
/// the Step no longer follows it; a refused resume leaves the Step paused.
#[tokio::test]
async fn a_prompt_the_session_refuses_is_not_followed() {
    let repo = FakeRepo::default();
    let refusing = FakeExecutor {
        steer_refused: true,
        ..FakeExecutor::default()
    };
    let service = terminal(&repo, &refusing);
    let (key, _) = step_in_a_session(&repo).await;

    let refused = service
        .steer_step(
            julia(),
            key.run_id,
            key.seq,
            TerminalRequest::Note("Rename it".to_string()),
        )
        .await;
    assert!(
        matches!(refused, Err(FactoryError::SessionUnreachable)),
        "{refused:?}"
    );
    assert_eq!(repo.steering(key).await.expect("read").prompt, None);

    // A pause whose stop the session did not take still holds the Step.
    service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Pause)
        .await
        .expect("the pause holds the step");
    let resume = service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Resume(None))
        .await;
    assert!(
        matches!(resume, Err(FactoryError::SessionUnreachable)),
        "{resume:?}"
    );
    let steering = repo.steering(key).await.expect("read");
    assert!(
        steering.paused,
        "a resume the agent never read leaves it paused"
    );
    assert_eq!(steering.prompt, None);

    let read = service.get_run(&julia(), key.run_id).await.expect("get");
    let undelivered: Vec<bool> = read.steps[0]
        .terminal
        .steers
        .iter()
        .map(|steer| steer.undelivered)
        .collect();
    assert_eq!(undelivered, [true, false, true]);
}

/// A Step its owner stopped, or that stayed paused past its time, ends
/// failed saying so, not with the word for any failed work; a paused Step
/// reads when it ends unless resumed.
#[tokio::test]
async fn a_stopped_or_timed_out_step_says_why_it_failed() {
    use crate::domain::models::{StepEnding, StepReport};
    use crate::domain::ports::StepService;
    use crate::domain::service::StepServiceImpl;
    use crate::domain::step_terminal::{
        PAUSE_RAN_OUT_OUTPUT, PAUSED_STEP_ENDS_AFTER, STOPPED_OUTPUT,
    };
    use factory_records::domain::models::Outcome;

    let repo = FakeRepo::default();
    let executor = FakeExecutor::default();
    let service = terminal(&repo, &executor);
    let steps = StepServiceImpl::new(repo.clone(), executor.clone());
    let (key, _) = step_in_a_session(&repo).await;

    service
        .steer_step(julia(), key.run_id, key.seq, TerminalRequest::Pause)
        .await
        .expect("pause");
    let paused = service.get_run(&julia(), key.run_id).await.expect("get");
    let step = &paused.steps[0];
    assert_eq!(
        step.terminal.ends_by,
        Some(step.started_at + PAUSED_STEP_ENDS_AFTER)
    );
    repo.pause_ran_out(key).await.expect("ran out");
    let failed = |output: &str| StepEnding {
        key,
        report: StepReport {
            outcome: Outcome::failed(),
            output: output.to_string(),
        },
    };
    steps
        .end_step(failed("The step's work failed."))
        .await
        .expect("end");
    let ended = service.get_run(&julia(), key.run_id).await.expect("get");
    assert_eq!(
        ended.steps[0].end.as_ref().map(|end| end.output.as_str()),
        Some(PAUSE_RAN_OUT_OUTPUT)
    );
    assert_eq!(ended.steps[0].terminal.ends_by, None);

    let (stopped, _) = step_in_a_session(&repo).await;
    service
        .steer_step(julia(), stopped.run_id, stopped.seq, TerminalRequest::Stop)
        .await
        .expect("stop");
    steps
        .end_step(StepEnding {
            key: stopped,
            ..failed("The step's work failed.")
        })
        .await
        .expect("end");
    let read = service
        .get_run(&julia(), stopped.run_id)
        .await
        .expect("get");
    assert_eq!(
        read.steps[0].end.as_ref().map(|end| end.output.as_str()),
        Some(STOPPED_OUTPUT)
    );
}

/// A session a Step of the owner's works in reads as that Step, for its
/// view to send them to the Run; to anyone else, and for a session no Step
/// works in, it is not found.
#[tokio::test]
async fn a_step_s_session_names_its_step_to_its_owner_only() {
    let repo = FakeRepo::default();
    let executor = FakeExecutor::default();
    let service = terminal(&repo, &executor);
    let (key, session_id) = step_in_a_session(&repo).await;

    let step = service
        .session_step(&julia(), session_id)
        .await
        .expect("julia's step");
    assert_eq!((step.run_id, step.seq), (key.run_id, key.seq));
    assert!(matches!(
        service.session_step(&teo(), session_id).await,
        Err(FactoryError::NotFound)
    ));
    assert!(matches!(
        service.session_step(&julia(), uuid::Uuid::now_v7()).await,
        Err(FactoryError::NotFound)
    ));
}
