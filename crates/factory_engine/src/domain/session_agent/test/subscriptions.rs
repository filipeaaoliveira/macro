//! A Step runs on its Run's owner's own Machines and API keys. An Agent
//! that names a vendor runs on the owner's Machines for it (their own CLI
//! login), then their own agents with a key, then their teams', passing
//! on for lack of quota in that order. A Machine's provider login runs
//! only its owner's Steps: an Agent bound to someone else's Machine, or to
//! a Macro computer signed in through Macro, is passed over, and a Step
//! with nothing it may run on waits, saying why.

use agent_api_keys::domain::{AgentOwner, KeyPlans};
use agent_session::domain::routines::RoutineActionStatus;
use bot_id::BotId;
use chrono::Utc;
use factory_records::domain::machines::{
    MachineLogin, OwnerBot, OwnerBotRuns, StepMachine, Vendor,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{
    BACKUP, FakeSessions, PRIMARY, SPARE, agent, name, no_quota, plan_task, recorded, the_plan,
};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::AgentTask;
use crate::domain::ports::{AgentFailure, Retryable, StepAgent};
use crate::domain::session_agent::SessionAgentError;

const JULIA: &str = "macro|julia@example.com";
const TEO: &str = "macro|teo@example.com";

/// Bind `bot` to the Machine `id`, named `name`, whose sessions run on
/// `login`.
fn bind(repo: &FakeRepo, bot: BotId, id: u128, name: &str, login: MachineLogin) {
    repo.machines.lock().expect("machines").insert(
        bot.as_uuid(),
        StepMachine {
            id: Uuid::from_u128(id),
            name: name.to_string(),
            step_slots: None,
            tags: Vec::new(),
            login,
        },
    );
}

/// `owner`'s bots for a vendor Agent, in order.
fn own(repo: &FakeRepo, owner: &str, bots: Vec<OwnerBot>) {
    repo.owner_bots
        .lock()
        .expect("owner bots")
        .insert(owner.to_string(), bots);
}

fn owner_bot(bot: BotId, model: &str, runs_on: OwnerBotRuns) -> OwnerBot {
    OwnerBot {
        bot: bot.as_uuid(),
        model: model.to_string(),
        runs_on,
    }
}

/// The bots of the sessions `sessions` was asked to open, in order.
fn prepared_bots(sessions: &FakeSessions) -> Vec<BotId> {
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    prepared.iter().map(|call| call.selection.bot_id).collect()
}

/// A planner that names a vendor: `claude`, whatever bot its owner has.
fn vendor_planner(repo: &FakeRepo, task: &AgentTask) {
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(
            task.key.run_id,
            serde_json::json!({
                "agents": { "claude": { "vendor": "anthropic" } },
                "roles": { "planner": ["claude"] }
            }),
        );
}

/// `task`, as `owner`'s.
fn owned_by(task: AgentTask, owner: &str) -> AgentTask {
    AgentTask {
        owner: MacroUserIdStr::try_from(owner.to_string()).expect("valid"),
        ..task
    }
}

const JULIAS_MACHINE_BOT: BotId = BotId::new_from_uuid(Uuid::from_u128(0x11));
const TEOS_MACHINE_BOT: BotId = BotId::new_from_uuid(Uuid::from_u128(0x22));
const JULIAS_KEY_BOT: BotId = BotId::new_from_uuid(Uuid::from_u128(0x33));
const TEAM_KEY_BOT: BotId = BotId::new_from_uuid(Uuid::from_u128(0x44));
const TEAM: Uuid = Uuid::from_u128(0x201);

/// Plans that include agents' own API keys for every owner but those in
/// `lapsed`.
struct Plans {
    lapsed: Vec<AgentOwner>,
}

#[async_trait::async_trait]
impl KeyPlans for Plans {
    async fn allows_keys(&self, owner: &AgentOwner) -> agent_api_keys::domain::Result<bool> {
        Ok(!self.lapsed.contains(owner))
    }
}

/// Every owner's plan includes keys.
fn paid() -> Plans {
    Plans { lapsed: Vec::new() }
}

/// Two people's Runs of one Definition each run on their own Subscription:
/// the vendor Agent takes Julia's Machine for Julia and Teo's for Teo.
#[tokio::test]
async fn each_persons_run_runs_on_their_own_subscription() {
    let repo = FakeRepo::default();
    bind(
        &repo,
        JULIAS_MACHINE_BOT,
        1,
        "julias-mac",
        MachineLogin::Owner(JULIA.to_string()),
    );
    bind(
        &repo,
        TEOS_MACHINE_BOT,
        2,
        "teos-mac",
        MachineLogin::Owner(TEO.to_string()),
    );
    own(
        &repo,
        JULIA,
        vec![owner_bot(
            JULIAS_MACHINE_BOT,
            "claude-opus-4-5",
            OwnerBotRuns::Subscription,
        )],
    );
    own(
        &repo,
        TEO,
        vec![owner_bot(
            TEOS_MACHINE_BOT,
            "claude-opus-4-5",
            OwnerBotRuns::Subscription,
        )],
    );
    let julias = plan_task(&repo).await;
    vendor_planner(&repo, &julias);
    let teos = owned_by(plan_task(&repo).await, TEO);
    vendor_planner(&repo, &teos);
    let (julias_key, teos_key) = (julias.key, teos.key);

    let for_julia = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    agent(&for_julia, &repo)
        .answer(julias)
        .await
        .expect("julia's plan");
    let for_teo = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    agent(&for_teo, &repo)
        .answer(teos)
        .await
        .expect("teo's plan");

    assert_eq!(prepared_bots(&for_julia), [JULIAS_MACHINE_BOT]);
    assert_eq!(prepared_bots(&for_teo), [TEOS_MACHINE_BOT]);
    assert_eq!(recorded(&repo, julias_key).1, Some(name("claude")));
    assert_eq!(recorded(&repo, teos_key).1, Some(name("claude")));
}

/// A vendor Agent takes its owner's Machine first, then their own agent
/// with a key, then their team's: each refused for lack of quota passes the
/// Step to the next, in that order, and bots of another vendor are not
/// taken.
#[tokio::test]
async fn a_vendor_agent_falls_back_from_the_subscription_to_the_owners_key_then_the_teams() {
    let repo = FakeRepo::default();
    bind(
        &repo,
        JULIAS_MACHINE_BOT,
        1,
        "julias-mac",
        MachineLogin::Owner(JULIA.to_string()),
    );
    own(
        &repo,
        JULIA,
        vec![
            owner_bot(
                JULIAS_MACHINE_BOT,
                "claude-opus-4-5",
                OwnerBotRuns::Subscription,
            ),
            owner_bot(SPARE, "gpt-5.5", OwnerBotRuns::Subscription),
            owner_bot(
                JULIAS_KEY_BOT,
                "anthropic/claude-sonnet-4-5",
                OwnerBotRuns::OwnKey(Vendor::Anthropic),
            ),
            owner_bot(
                TEAM_KEY_BOT,
                "anthropic/claude-sonnet-4-5",
                OwnerBotRuns::TeamKey(Vendor::Anthropic, TEAM),
            ),
        ],
    );
    let task = plan_task(&repo).await;
    vendor_planner(&repo, &task);
    let key = task.key;
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota(), no_quota()]);

    agent(&sessions, &repo)
        .with_key_plans(paid())
        .answer(task)
        .await
        .expect("plan");

    assert_eq!(
        prepared_bots(&sessions),
        [JULIAS_MACHINE_BOT, JULIAS_KEY_BOT, TEAM_KEY_BOT]
    );
    let passed: Vec<String> = recorded(&repo, key)
        .2
        .iter()
        .map(|passed| passed.agent.as_str().to_string())
        .collect();
    assert_eq!(passed, ["claude", "claude"]);
}

/// A Definition whose planner is `primary`, bound to Teo's Machine, then
/// `backup`.
fn shared_machine(repo: &FakeRepo, task: &AgentTask, backup_login: Option<MachineLogin>) {
    bind(
        repo,
        PRIMARY,
        1,
        "teos-mac",
        MachineLogin::Owner(TEO.to_string()),
    );
    if let Some(login) = backup_login {
        bind(repo, BACKUP, 2, "computer", login);
    }
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(
            task.key.run_id,
            serde_json::json!({
                "agents": {
                    "primary": { "bot": PRIMARY.as_uuid() },
                    "backup": { "bot": BACKUP.as_uuid() }
                },
                "roles": { "planner": ["primary", "backup"] }
            }),
        );
}

/// A fixed-bot Agent whose Machine someone else paired never runs Julia's
/// Step, even shared with her: it is passed over like an Agent whose
/// Machine lacks its tags, and the next one takes the Step.
#[tokio::test]
async fn an_agent_on_someone_elses_machine_is_passed_over() {
    let repo = FakeRepo::default();
    let task = plan_task(&repo).await;
    shared_machine(&repo, &task, Some(MachineLogin::MacroCredits));
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    agent(&sessions, &repo).answer(task).await.expect("plan");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
}

/// With no Agent its owner may run on, a Step waits, saying why, and opens
/// no session: here one Agent is on someone else's Machine and the other
/// on a Macro computer signed in to a provider through Macro. Its attempt
/// fails for a retry once its time is up, as a wait for a Machine.
#[tokio::test(start_paused = true)]
async fn a_step_with_no_machine_or_key_of_its_owners_waits_saying_why() {
    let repo = FakeRepo::default();
    let mut task = plan_task(&repo).await;
    shared_machine(&repo, &task, Some(MachineLogin::HostedSignIn));
    task.answer_by = Some(Utc::now() + chrono::Duration::seconds(60));
    let key = task.key;
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    let error = agent(&sessions, &repo)
        .answer(task)
        .await
        .expect_err("nothing to run on");

    assert!(
        matches!(&error, SessionAgentError::WaitingForMachineOrKey(_)),
        "{error:?}"
    );
    assert!(error.retryable());
    assert!(error.waits_for_machine());
    assert!(prepared_bots(&sessions).is_empty());
    let reason = repo
        .slot_wait_reasons
        .lock()
        .expect("reasons")
        .get(&key)
        .cloned()
        .expect("a reason");
    assert!(reason.contains("teos-mac, which is not yours"), "{reason}");
    assert!(
        reason.contains("signed in to a provider through Macro"),
        "{reason}"
    );
}

/// A vendor Agent whose owner has no Machine or key for it waits, saying
/// so, and runs once the owner pairs a Machine.
#[tokio::test(start_paused = true)]
async fn a_vendor_agent_with_nothing_of_its_owners_waits_until_they_pair_a_machine() {
    let repo = FakeRepo::default();
    let mut task = plan_task(&repo).await;
    vendor_planner(&repo, &task);
    task.answer_by = Some(Utc::now() + chrono::Duration::seconds(60));
    let key = task.key;
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    let error = agent(&sessions, &repo)
        .answer(task.clone())
        .await
        .expect_err("nothing to run on");

    assert!(
        matches!(&error, SessionAgentError::WaitingForMachineOrKey(why) if why.contains("your own Anthropic machine or API key")),
        "{error:?}"
    );
    assert!(prepared_bots(&sessions).is_empty());

    bind(
        &repo,
        JULIAS_MACHINE_BOT,
        1,
        "julias-mac",
        MachineLogin::Owner(JULIA.to_string()),
    );
    own(
        &repo,
        JULIA,
        vec![owner_bot(
            JULIAS_MACHINE_BOT,
            "claude-opus-4-5",
            OwnerBotRuns::Subscription,
        )],
    );
    task.answer_by = None;
    agent(&sessions, &repo).answer(task).await.expect("plan");

    assert_eq!(prepared_bots(&sessions), [JULIAS_MACHINE_BOT]);
    assert_eq!(
        repo.slot_wait_reasons.lock().expect("reasons").get(&key),
        None
    );
}

/// A key whose owner's plan does not include keys is not taken, so its
/// turns never fall back to Macro credits: the team's key takes the Step
/// when only Julia's plan lapsed, and with every plan lapsed the Step waits,
/// saying the plan does not include keys.
#[tokio::test(start_paused = true)]
async fn a_key_whose_plan_lapsed_is_not_taken() {
    let repo = FakeRepo::default();
    own(
        &repo,
        JULIA,
        vec![
            owner_bot(
                JULIAS_KEY_BOT,
                "anthropic/claude-sonnet-4-5",
                OwnerBotRuns::OwnKey(Vendor::Anthropic),
            ),
            owner_bot(
                TEAM_KEY_BOT,
                "anthropic/claude-sonnet-4-5",
                OwnerBotRuns::TeamKey(Vendor::Anthropic, TEAM),
            ),
        ],
    );
    let julia = MacroUserIdStr::try_from(JULIA.to_string()).expect("valid");
    let task = plan_task(&repo).await;
    vendor_planner(&repo, &task);
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    agent(&sessions, &repo)
        .with_key_plans(Plans {
            lapsed: vec![AgentOwner::User(julia.clone())],
        })
        .answer(task)
        .await
        .expect("plan");
    assert_eq!(prepared_bots(&sessions), [TEAM_KEY_BOT]);

    let mut task = plan_task(&repo).await;
    vendor_planner(&repo, &task);
    task.answer_by = Some(Utc::now() + chrono::Duration::seconds(60));
    let waiting = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let error = agent(&waiting, &repo)
        .with_key_plans(Plans {
            lapsed: vec![AgentOwner::User(julia), AgentOwner::Team(TEAM)],
        })
        .answer(task)
        .await
        .expect_err("no key to run on");
    assert!(
        matches!(&error, SessionAgentError::WaitingForMachineOrKey(why) if why.contains("does not include API keys")),
        "{error:?}"
    );
    assert!(prepared_bots(&waiting).is_empty());
}

/// A Step that waited for a Machine or key past the time a Step may still
/// take a slot stops waiting for good, saying why.
#[tokio::test(start_paused = true)]
async fn a_step_waiting_past_its_time_for_a_machine_or_key_stops() {
    let repo = FakeRepo::default();
    let task = plan_task(&repo).await;
    vendor_planner(&repo, &task);
    repo.steps
        .lock()
        .expect("steps")
        .iter_mut()
        .filter(|(run, step)| *run == task.key.run_id && step.seq == task.key.seq)
        .for_each(|(_, step)| step.started_at = Utc::now() - chrono::Duration::minutes(100));
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    let error = agent(&sessions, &repo)
        .answer(task)
        .await
        .expect_err("too late");

    assert!(
        matches!(&error, SessionAgentError::MachineOrKeyTimeRanOut(_)),
        "{error:?}"
    );
    assert!(!error.retryable());
    assert!(error.waits_for_machine());
}

/// A session claimed, unopened, for an Agent on someone else's Machine,
/// as before the rule that a Machine runs only its owner's Steps, is handed
/// to the next fitting Agent, recorded passed over as not the owner's; with
/// none, the Step waits, saying why.
#[tokio::test(start_paused = true)]
async fn a_claimed_session_on_someone_elses_machine_is_handed_on_or_waits() {
    use factory_records::domain::ports::RunRepository;

    use super::planner;
    use crate::domain::session_agent::{AgentBot, NOT_THE_OWNERS, SessionAgent};

    let repo = FakeRepo::default();
    let task = plan_task(&repo).await;
    shared_machine(&repo, &task, None);
    let claim = |task: AgentTask| {
        let repo = repo.clone();
        async move {
            repo.claim_session(
                task.key,
                SessionAgent::<FakeSessions, FakeRepo>::new_session(&AgentBot {
                    agent: name("primary"),
                    bot: PRIMARY,
                }),
                &planner(),
                &task.outcomes,
            )
            .await
            .expect("an earlier attempt's claim");
        }
    };
    claim(task.clone()).await;
    let key = task.key;
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    agent(&sessions, &repo).answer(task).await.expect("plan");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
    let reasons: Vec<String> = recorded(&repo, key)
        .2
        .iter()
        .map(|passed| passed.reason.clone())
        .collect();
    assert_eq!(reasons, [NOT_THE_OWNERS]);

    // With `primary` alone, the Step waits.
    let mut alone = plan_task(&repo).await;
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(
            alone.key.run_id,
            serde_json::json!({
                "agents": { "primary": { "bot": PRIMARY.as_uuid() } },
                "roles": { "planner": ["primary"] }
            }),
        );
    claim(alone.clone()).await;
    alone.answer_by = Some(Utc::now() + chrono::Duration::seconds(60));
    let waiting = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let error = agent(&waiting, &repo)
        .answer(alone)
        .await
        .expect_err("nothing to run on");
    assert!(
        matches!(&error, SessionAgentError::WaitingForMachineOrKey(why) if why.contains("teos-mac, which is not yours")),
        "{error:?}"
    );
    assert!(prepared_bots(&waiting).is_empty());
}
