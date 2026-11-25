use agent_session::domain::routines::RoutineActionStatus;
use factory_records::domain::models::{EntryId, HandBack, Outcome, StepKey};
use factory_records::domain::ports::RunRepository;

use super::{FakeSessions, agent, plan_task, recorded, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::AgentTask;
use crate::domain::ports::{Retryable, StepAgent};
use crate::domain::session_agent::SessionAgentError;

/// The agent's question, handed back for the Step's first prompt.
fn the_question() -> HandBack {
    HandBack {
        outcome: Outcome::question(),
        output: "CSV or JSON?".to_string(),
    }
}

/// A `plan` Step whose agent asked a question, ended, and the Step 2 that
/// goes on from it, started.
async fn asked(repo: &FakeRepo, sessions: &FakeSessions) -> (AgentTask, AgentTask) {
    let task = AgentTask {
        outcomes: vec![Outcome::done(), Outcome::question()],
        ..plan_task(repo).await
    };
    let answer = agent(sessions, repo)
        .answer(task.clone())
        .await
        .expect("answer");
    assert_eq!(answer.outcome, Outcome::question());
    repo.end_step(task.key, &answer.outcome, &answer.text)
        .await
        .expect("end");
    let going_on = StepKey {
        run_id: task.key.run_id,
        seq: 2,
    };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(going_on, &entry, 1).await.expect("start");
    let next = AgentTask {
        key: going_on,
        prompt: "The owner answered: CSV".to_string(),
        resumes: Some(task.key.seq),
        ..task.clone()
    };
    (task, next)
}

/// A Step that goes on from its agent's question opens no session: it
/// prompts the asking Step's, under a prompt of its own, and its answer is
/// what the agent handed back for that prompt. A retry asks under the same
/// prompt, which the session refuses, and reads the same answer. A stop of
/// the Step then watches that prompt, in the session it opened before.
#[tokio::test]
async fn a_step_going_on_from_a_question_prompts_the_asking_step_s_session() {
    let repo = FakeRepo::default();
    let mut sessions =
        FakeSessions::new(&repo, Some(the_question()), RoutineActionStatus::Succeeded);
    sessions.follow_up_handed = Some(the_plan());
    let (task, next) = asked(&repo, &sessions).await;
    let asking = repo
        .claimed_session(task.key)
        .await
        .expect("read")
        .expect("claimed");
    let agent = agent(&sessions, &repo);

    let answer = agent.answer(next.clone()).await.expect("go on");
    let again = agent.answer(next.clone()).await.expect("again");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(answer.text, the_plan().output);
    assert_eq!(again, answer);
    assert_eq!(sessions.prepares(), 1);
    let calls = sessions.follow_up_calls.lock().expect("follow-ups").clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].action.session_id.as_uuid(), asking.session_id);
    assert_eq!(calls[1].action.action_id, calls[0].action.action_id);
    assert_ne!(calls[0].action.action_id.as_uuid(), asking.action_id);
    assert!(calls[0].prompt.starts_with("The owner answered: CSV"));
    assert!(calls[0].prompt.contains("`done`, `question`"));
    let stopping = repo
        .stopping_session(next.key, std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    assert_eq!(stopping.session.session_id, asking.session_id);
    assert_eq!(
        stopping.session.action_id,
        calls[0].action.action_id.as_uuid()
    );
    assert!(stopping.opened);
}

/// A Step whose asking Step holds no session (a Step no agent did) has
/// nothing to go on in, and fails for good.
#[tokio::test]
async fn a_step_going_on_from_a_step_with_no_session_fails_for_good() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let next = AgentTask {
        resumes: Some(task.key.seq),
        ..task
    };

    let failed = agent(&sessions, &repo).answer(next).await;

    let Err(error) = failed else {
        panic!("there is no session to go on in");
    };
    assert!(matches!(error, SessionAgentError::NoSession));
    assert!(!error.retryable());
    assert_eq!(sessions.prepares(), 0);
}

/// A session the harness cannot resume once the owner answered (its
/// runtime stays disconnected) is lost: it is deleted, the Step records
/// its Agent passed over for it, and goes on in a new session of its
/// Role's first Agent, prompted with the whole Step afresh. A retry goes
/// on in that new session.
#[tokio::test(start_paused = true)]
async fn a_session_lost_while_waiting_goes_on_afresh_in_a_new_one() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_question()), RoutineActionStatus::Succeeded);
    let (task, next) = asked(&repo, &sessions).await;
    let asking = repo
        .claimed_session(task.key)
        .await
        .expect("read")
        .expect("claimed");
    let mut losing = sessions.clone();
    losing.follow_up_loses = true;
    losing.handed = Some(the_plan());
    let next = AgentTask {
        afresh: Some("Plan the change, afresh".to_string()),
        ..next
    };
    let agent = agent(&losing, &repo);

    let answer = agent.answer(next.clone()).await.expect("go on afresh");
    let again = agent.answer(next.clone()).await.expect("again");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(answer.text, the_plan().output);
    assert_eq!(again, answer);
    let deleted = losing.deleted.lock().expect("deleted").clone();
    assert_eq!(deleted[0].session_id.as_uuid(), asking.session_id);
    let fresh = repo
        .claimed_session(next.key)
        .await
        .expect("read")
        .expect("claimed");
    assert_ne!(fresh.session_id, asking.session_id);
    let prompts = losing.prompt_calls.lock().expect("prompts").clone();
    let last = prompts.last().expect("a fresh prompt");
    assert_eq!(last.action.session_id.as_uuid(), fresh.session_id);
    assert!(last.prompt.starts_with("Plan the change, afresh"));
    let (_, _, passed) = recorded(&repo, next.key);
    assert_eq!(
        passed
            .iter()
            .map(|passed| passed.reason.as_str())
            .collect::<Vec<_>>(),
        ["session_lost"]
    );
}

/// A session lost with no whole prompt to go on afresh with fails the
/// Step for good, saying the session is lost.
#[tokio::test(start_paused = true)]
async fn a_session_lost_with_nothing_to_go_on_afresh_with_fails_for_good() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_question()), RoutineActionStatus::Succeeded);
    let (_, next) = asked(&repo, &sessions).await;
    let mut losing = sessions.clone();
    losing.follow_up_loses = true;

    let failed = agent(&losing, &repo).answer(next).await;

    let Err(error) = failed else {
        panic!("the session is lost");
    };
    assert!(matches!(error, SessionAgentError::SessionLost));
    assert!(!error.retryable());
}

/// A session lost after its question goes on afresh with the first Agent
/// of the Role whose Machine has the tags it asks for: the Role's first
/// Agent, whose Machine lost its `gpu` tag meanwhile, is passed over, and
/// the Step holds a slot on the Machine its new session opens on.
#[tokio::test(start_paused = true)]
async fn a_lost_session_goes_on_afresh_with_an_agent_whose_machine_has_the_tags() {
    use factory_records::domain::machines::{MachineLogin, StepMachine};
    use uuid::Uuid;

    use super::{BACKUP, PRIMARY};

    let m3 = Uuid::from_u128(0x0a3);
    let m5 = Uuid::from_u128(0x0a5);
    let repo = FakeRepo::default();
    let machine = |id: Uuid, name: &str, tags: &[&str]| StepMachine {
        id,
        name: name.to_string(),
        step_slots: Some(1),
        tags: tags.iter().map(ToString::to_string).collect(),
        login: MachineLogin::Owner("macro|julia@example.com".to_string()),
    };
    repo.machines
        .lock()
        .expect("machines")
        .insert(PRIMARY.as_uuid(), machine(m3, "m3", &["gpu"]));
    repo.machines
        .lock()
        .expect("machines")
        .insert(BACKUP.as_uuid(), machine(m5, "m5", &["gpu"]));
    let sessions = FakeSessions::new(&repo, Some(the_question()), RoutineActionStatus::Succeeded);
    let (_, next) = asked(&repo, &sessions).await;
    let kept = serde_json::json!({
        "agents": {
            "primary": { "bot": PRIMARY.as_uuid(), "machine_tags": ["gpu"] },
            "backup": { "bot": BACKUP.as_uuid(), "machine_tags": ["gpu"] }
        },
        "roles": { "planner": ["primary", "backup"] }
    });
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(next.key.run_id, kept);
    repo.machines
        .lock()
        .expect("machines")
        .insert(PRIMARY.as_uuid(), machine(m3, "m3", &["rust-build"]));
    let mut losing = sessions.clone();
    losing.follow_up_loses = true;
    losing.handed = Some(the_plan());
    let next = AgentTask {
        afresh: Some("Plan the change, afresh".to_string()),
        ..next
    };

    agent(&losing, &repo)
        .answer(next.clone())
        .await
        .expect("go on afresh");

    let prepared = losing.prepared_calls.lock().expect("prepared").clone();
    assert_eq!(
        prepared.last().expect("a fresh session").selection.bot_id,
        BACKUP
    );
    assert_eq!(
        repo.machine_slots.lock().expect("slots").get(&next.key),
        Some(&m5)
    );
}

/// A session lost after its question, whose Role's Agents are all on a
/// Machine someone else paired, waits to go on afresh, saying why, rather
/// than fail.
#[tokio::test(start_paused = true)]
async fn a_lost_session_with_no_agent_of_the_owners_waits_saying_why() {
    use factory_records::domain::machines::{MachineLogin, StepMachine};
    use uuid::Uuid;

    use super::{BACKUP, PRIMARY};

    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_question()), RoutineActionStatus::Succeeded);
    let (_, next) = asked(&repo, &sessions).await;
    for bot in [PRIMARY, BACKUP] {
        repo.machines.lock().expect("machines").insert(
            bot.as_uuid(),
            StepMachine {
                id: Uuid::from_u128(0x0a7),
                name: "teos-mac".to_string(),
                step_slots: None,
                tags: Vec::new(),
                login: MachineLogin::Owner("macro|teo@example.com".to_string()),
            },
        );
    }
    let mut losing = sessions.clone();
    losing.follow_up_loses = true;
    losing.handed = Some(the_plan());
    let next = AgentTask {
        afresh: Some("Plan the change, afresh".to_string()),
        ..next
    };

    let error = agent(&losing, &repo)
        .answer(next)
        .await
        .expect_err("nothing to go on in");

    assert!(
        matches!(&error, SessionAgentError::WaitingForMachineOrKey(why) if why.contains("teos-mac, which is not yours")),
        "{error:?}"
    );
    assert!(error.retryable());
}
