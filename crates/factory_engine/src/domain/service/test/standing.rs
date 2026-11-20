//! Standing Roles: under a Factory Definition with a `standing` section, an
//! agent acting for a Run's owner starts, cancels or links the Run, or
//! answers its plan or Blockers Gate, only when its bot fills a Standing
//! Role that allows the act, and the Run keeps which bot and Standing Role
//! did it. With no `standing` section, every act goes on as before.

use std::sync::Arc;

use bot_id::BotId;
use factory_records::domain::evaluation::AgentChoices;
use factory_records::domain::models::{
    AgentName, EntryId, NewRun, Outcome, Repository, Role, RunId, RunInsert, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_acts::{StandingAct, StandingActor, StandingRoleName};
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, OrchestratorKinds, RulesSource, RunRules,
};
use serde_json::json;
use uuid::Uuid;

use super::definition::{DEFINITION_REPO, FIRST, github_at, team_repo};
use super::{FakeExecutor, FakeWorkflows, code_service, julia, teo};
use crate::domain::evaluation::EvaluationDefinition;
use crate::domain::evaluation::insights::{EvaluationService, EvaluationServiceImpl, SuiteError};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::gate::{AnswerGate, GateAnswered, StandingAnswer};
use crate::domain::models::{ActingAgent, Answerer, FactoryError, PersonOnly};
use crate::domain::ports::{FactoryService, StepService};
use crate::domain::service::StepServiceImpl;

pub(super) const PRIMARY_BOT: Uuid = Uuid::from_u128(0xc1a0);
pub(super) const BACKUP_BOT: Uuid = Uuid::from_u128(0xc0de);

/// The Agents `primary` and `backup`, with `standing` as their Standing
/// Roles when given.
fn agents(standing: Option<serde_json::Value>) -> serde_json::Value {
    let mut agents = json!({
        "agents": {
            "primary": { "bot": PRIMARY_BOT.to_string() },
            "backup": { "bot": BACKUP_BOT.to_string() }
        },
        "roles": {
            "planner": ["primary"],
            "writer": ["primary"],
            "reviewer": ["backup"],
            "fixer": ["primary"]
        }
    });
    if let Some(standing) = standing {
        agents["standing"] = standing;
    }
    agents
}

/// `gatekeeper` answers plans and Blockers as `primary`; `intake` starts
/// and links as `backup`; `operator` cancels as `backup`.
fn split_roles() -> serde_json::Value {
    json!({
        "gatekeeper": { "acts": ["answer_plan", "answer_blockers"], "agents": ["primary"] },
        "intake": { "acts": ["start", "link"], "agents": ["backup"] },
        "operator": { "acts": ["cancel"], "agents": ["backup"] }
    })
}

/// `primary` fills every Standing Role.
fn one_agent_everywhere() -> serde_json::Value {
    json!({
        "all": {
            "acts": ["start", "cancel", "link", "answer_plan", "answer_blockers"],
            "agents": ["primary"]
        }
    })
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

fn standing_role(name: &str) -> StandingRoleName {
    StandingRoleName::try_from(name.to_string()).expect("valid")
}

/// A new agent session of Julia's, run by `bot`.
pub(super) fn session_of(repo: &FakeRepo, bot: Uuid) -> Uuid {
    let session = Uuid::now_v7();
    repo.add_session_bot(session, julia().as_ref(), bot);
    session
}

/// Rules that let the Orchestrator approve plans and send Blockers to a
/// fix.
fn approving_rules() -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            orchestrator: OrchestratorKinds {
                plan: true,
                blockers: true,
            },
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// A `code` Run of Julia's that kept `kept_agents` as its Definition's
/// Agents (none: it started under no Definition), whose Step 1 at `entry`
/// ended `ended` and asks `answers` on a Gate.
async fn waiting_run(
    repo: &FakeRepo,
    kept_agents: Option<serde_json::Value>,
    entry: &str,
    ended: &str,
    answers: &[&str],
) -> RunId {
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Fix the login bug",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: Some(&Repository {
                url: "https://github.com/macro/app".to_string(),
                base_branch: "main".to_string(),
            }),
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: approving_rules(),
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    if let Some(kept) = kept_agents {
        repo.definition_agents
            .lock()
            .expect("definition agents")
            .insert(run.id, kept);
    }
    let key = StepKey {
        run_id: run.id,
        seq: 1,
    };
    let entry = EntryId::try_from(entry.to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.end_step(key, &outcome(ended), "Step output")
        .await
        .expect("end");
    let answers: Vec<Outcome> = answers.iter().map(|answer| outcome(answer)).collect();
    repo.open_gate(key, "Go on?", &answers).await.expect("open");
    run.id
}

/// What `service` says to the agent session `session` answering `answer`
/// on Step 1 of `run`.
async fn answer_from(
    service: &impl FactoryService,
    run: RunId,
    answer: &str,
    session: Uuid,
) -> Result<(), FactoryError> {
    service
        .answer_gate(
            julia(),
            run,
            1,
            outcome(answer),
            None,
            None,
            Answerer::Agent {
                session_id: session,
            },
        )
        .await
}

/// With `gatekeeper` filled by `primary`'s bot, a plan approval from
/// another bot's session is refused, naming `gatekeeper`; from `primary`'s
/// it is taken, with the bot, its Agent and the Standing Role for the
/// worker to keep. A Blockers Gate takes the same check.
#[tokio::test]
async fn only_a_bot_in_a_standing_role_that_allows_it_answers_a_gate() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let plan = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;
    let blockers = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "review",
        "changes",
        &["fix", "stop"],
    )
    .await;
    let backup = session_of(&repo, BACKUP_BOT);
    let primary = session_of(&repo, PRIMARY_BOT);

    let refused = answer_from(&service, plan, "approve", backup).await;
    let sent_back = answer_from(&service, plan, "revise", backup).await;
    let unknown = answer_from(&service, plan, "approve", Uuid::now_v7()).await;
    let blockers_refused = answer_from(&service, blockers, "fix", backup).await;
    let taken = answer_from(&service, plan, "approve", primary).await;
    let blockers_taken = answer_from(&service, blockers, "fix", primary).await;

    for refusal in [refused, sent_back, unknown] {
        let Err(FactoryError::NotInStandingRole { act, allowing }) = refusal else {
            panic!("a bot in no standing role that answers plans is refused");
        };
        assert_eq!(act, StandingAct::AnswerPlan);
        assert_eq!(allowing, [standing_role("gatekeeper")]);
    }
    assert!(matches!(
        blockers_refused,
        Err(FactoryError::NotInStandingRole {
            act: StandingAct::AnswerBlockers,
            ..
        })
    ));
    let message = FactoryError::NotInStandingRole {
        act: StandingAct::AnswerPlan,
        allowing: vec![standing_role("gatekeeper")],
    }
    .to_string();
    assert!(message.contains("answer_plan") && message.contains("gatekeeper"));
    taken.expect("the gatekeeper's approval");
    blockers_taken.expect("the gatekeeper's fix");

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers.len(), 2);
    assert_eq!(
        answers[0].standing,
        Some(StandingAnswer {
            act: StandingAct::AnswerPlan,
            by: StandingActor {
                session_id: Some(primary),
                bot_id: PRIMARY_BOT,
                agent: AgentName::try_from("primary".to_string()).expect("valid"),
                standing_role: standing_role("gatekeeper"),
            },
        })
    );
    assert_eq!(
        answers[1].standing.as_ref().map(|standing| standing.act),
        Some(StandingAct::AnswerBlockers)
    );
}

/// No agent answers a merge Gate: under Standing Roles, not even with a
/// bot that fills every one, whatever the answer; without them, it still
/// never approves one.
#[tokio::test]
async fn no_agent_answers_a_merge_gate_whatever_its_standing_roles() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let everywhere = waiting_run(
        &repo,
        Some(agents(Some(one_agent_everywhere()))),
        "merge-gate",
        "done",
        &["approve", "revise"],
    )
    .await;
    let open = waiting_run(&repo, None, "merge-gate", "done", &["approve", "revise"]).await;
    let primary = session_of(&repo, PRIMARY_BOT);

    let approved = answer_from(&service, everywhere, "approve", primary).await;
    let sent_back = answer_from(&service, everywhere, "revise", primary).await;
    let approved_without = answer_from(&service, open, "approve", primary).await;

    for refusal in [approved, sent_back, approved_without] {
        assert!(
            matches!(
                refusal,
                Err(FactoryError::ReservedForAPerson(PersonOnly::Merge))
            ),
            "{refusal:?}"
        );
    }
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

/// Without Standing Roles an agent still sends a merge back: `revise` on a
/// merge Gate of a Run under no Definition, or under one with no
/// `standing` section, is taken as before, with no Standing Role kept.
#[tokio::test]
async fn without_standing_roles_an_agent_sends_a_merge_back() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let no_definition =
        waiting_run(&repo, None, "merge-gate", "done", &["approve", "revise"]).await;
    let no_section = waiting_run(
        &repo,
        Some(agents(None)),
        "merge-gate",
        "done",
        &["approve", "revise"],
    )
    .await;

    answer_from(&service, no_definition, "revise", Uuid::now_v7())
        .await
        .expect("a merge sent back under no definition");
    answer_from(&service, no_section, "revise", Uuid::now_v7())
        .await
        .expect("a merge sent back under no standing section");

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers.len(), 2);
    assert!(answers.iter().all(|answer| answer.standing.is_none()));
    assert!(repo.standing_acts.lock().expect("acts").is_empty());
}

/// A Run under no Definition, or one whose Definition has no `standing`
/// section, takes an agent's answer as before: from any session of the
/// owner's, with no Standing Role kept.
#[tokio::test]
async fn without_standing_roles_an_agent_answers_as_before() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let no_definition = waiting_run(&repo, None, "plan", "done", &["approve", "revise"]).await;
    let no_section = waiting_run(
        &repo,
        Some(agents(None)),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;

    answer_from(&service, no_definition, "approve", Uuid::now_v7())
        .await
        .expect("an approval under no definition");
    answer_from(&service, no_section, "approve", Uuid::now_v7())
        .await
        .expect("an approval under no standing section");

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers.len(), 2);
    assert!(answers.iter().all(|answer| answer.standing.is_none()));
    assert!(repo.standing_acts.lock().expect("acts").is_empty());
}

/// The worker keeps the bot, Agent and Standing Role an answer came with,
/// once however often its activity runs, and the Run API shows them.
#[tokio::test]
async fn the_recorded_answer_keeps_its_standing_role() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run_id = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;
    let primary = session_of(&repo, PRIMARY_BOT);
    let by = StandingActor {
        session_id: Some(primary),
        bot_id: PRIMARY_BOT,
        agent: AgentName::try_from("primary".to_string()).expect("valid"),
        standing_role: standing_role("gatekeeper"),
    };
    let answered = GateAnswered {
        key: StepKey { run_id, seq: 1 },
        answer: AnswerGate {
            seq: 1,
            answer: outcome("approve"),
            note: None,
            user_id: julia(),
            approved_control_files: None,
            agent_session: Some(primary),
            standing: Some(StandingAnswer {
                act: StandingAct::AnswerPlan,
                by: by.clone(),
            }),
        },
    };
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());

    steps.answer_gate(answered.clone()).await.expect("record");
    steps.answer_gate(answered).await.expect("record again");

    let detail = service.get_run(&julia(), run_id).await.expect("read");
    assert_eq!(detail.standing_acts.len(), 1);
    let kept = &detail.standing_acts[0];
    assert_eq!(
        (kept.act, kept.seq, kept.bot_id, kept.session_id),
        (StandingAct::AnswerPlan, Some(1), PRIMARY_BOT, Some(primary))
    );
    assert_eq!(kept.agent, by.agent);
    assert_eq!(kept.standing_role, by.standing_role);
}

/// Cancelling and linking a Run under Standing Roles take a bot that fills
/// one allowing it, and the Run keeps who did; the owner in person, and
/// any agent under no `standing` section, do both as before.
#[tokio::test]
async fn cancel_and_link_take_a_bot_in_a_standing_role_that_allows_them() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let run_id = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;
    let open = waiting_run(&repo, Some(agents(None)), "plan", "done", &["approve"]).await;
    let primary = session_of(&repo, PRIMARY_BOT);
    let backup = session_of(&repo, BACKUP_BOT);

    let link_refused = service
        .link_task(&julia(), run_id, task, Some(ActingAgent::Session(primary)))
        .await;
    let cancel_refused = service
        .cancel_run(&julia(), run_id, Some(ActingAgent::Session(primary)), false)
        .await;
    let in_person = service.link_task(&julia(), run_id, task, None).await;
    let linked = service
        .link_task(&julia(), run_id, task, Some(ActingAgent::Session(backup)))
        .await;
    let cancelled = service
        .cancel_run(&julia(), run_id, Some(ActingAgent::Session(backup)), false)
        .await;
    let open_linked = service
        .link_task(&julia(), open, task, Some(ActingAgent::Session(primary)))
        .await;
    let open_cancelled = service
        .cancel_run(&julia(), open, Some(ActingAgent::Session(primary)), false)
        .await;

    let Err(FactoryError::NotInStandingRole { act, allowing }) = link_refused else {
        panic!("a bot in no standing role that links is refused");
    };
    assert_eq!(
        (act, allowing),
        (StandingAct::Link, vec![standing_role("intake")])
    );
    let Err(FactoryError::NotInStandingRole { act, allowing }) = cancel_refused else {
        panic!("a bot in no standing role that cancels is refused");
    };
    assert_eq!(
        (act, allowing),
        (StandingAct::Cancel, vec![standing_role("operator")])
    );
    in_person.expect("the owner links in person");
    linked.expect("the intake links");
    cancelled.expect("the operator cancels");
    open_linked.expect("any agent links under no standing section");
    open_cancelled.expect("any agent cancels under no standing section");

    // The refused cancel reached no workflow; the operator's and the open
    // Run's did.
    let cancels = workflows.cancelled.lock().expect("cancels").clone();
    assert_eq!(cancels, [run_id, open]);
    let detail = service.get_run(&julia(), run_id).await.expect("read");
    let kept: Vec<_> = detail
        .standing_acts
        .iter()
        .map(|act| (act.act, act.standing_role.as_str().to_string(), act.bot_id))
        .collect();
    assert_eq!(
        kept,
        [
            (StandingAct::Link, "intake".to_string(), BACKUP_BOT),
            (StandingAct::Cancel, "operator".to_string(), BACKUP_BOT),
        ]
    );
    let open_detail = service.get_run(&julia(), open).await.expect("read");
    assert!(open_detail.standing_acts.is_empty());
}

/// A cancel the workflow does not take keeps no one as its sender: not one
/// whose workflow already ended the Run, nor one Temporal fails. A later
/// cancel that goes through is then the one the Run keeps.
#[tokio::test]
async fn a_cancel_that_is_not_taken_keeps_no_standing_act() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let ended = FakeWorkflows {
        completed: true,
        ..FakeWorkflows::default()
    };
    let failing = FakeWorkflows {
        cancel_fails: true,
        ..FakeWorkflows::default()
    };
    let taking = FakeWorkflows::default();
    let run_id = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;
    let backup = session_of(&repo, BACKUP_BOT);

    let too_late = code_service(&repo, &ended, &github)
        .cancel_run(&julia(), run_id, Some(ActingAgent::Session(backup)), false)
        .await;
    let failed = code_service(&repo, &failing, &github)
        .cancel_run(&julia(), run_id, Some(ActingAgent::Session(backup)), false)
        .await;

    assert!(
        matches!(too_late, Err(FactoryError::RunEnded)),
        "{too_late:?}"
    );
    assert!(failed.is_err(), "{failed:?}");
    assert!(repo.standing_acts.lock().expect("acts").is_empty());

    let other = session_of(&repo, BACKUP_BOT);
    let service = code_service(&repo, &taking, &github);
    service
        .cancel_run(&julia(), run_id, Some(ActingAgent::Session(other)), false)
        .await
        .expect("the operator cancels");
    let detail = service.get_run(&julia(), run_id).await.expect("read");
    let kept: Vec<_> = detail
        .standing_acts
        .iter()
        .map(|act| (act.act, act.session_id))
        .collect();
    assert_eq!(kept, [(StandingAct::Cancel, Some(other))]);
}

/// GitHub holding a Definition at `FIRST` with one Flow, `notes`, and the
/// split Standing Roles over the `primary` and `backup` Agents.
pub(super) fn notes_definition() -> FakeGithub {
    notes_definition_with(json!({
        "agents": {
            "primary": { "bot": PRIMARY_BOT.to_string() },
            "backup": { "bot": BACKUP_BOT.to_string() }
        },
        "roles": { "writer": ["primary"] },
        "standing": split_roles()
    }))
}

/// GitHub holding a Definition at `FIRST` with one Flow, `notes`, and
/// `agents` as its Agents file.
pub(super) fn notes_definition_with(agents: serde_json::Value) -> FakeGithub {
    let github = github_at(FIRST);
    let flow = r#"{
        "entries": [{
            "id": "reply",
            "step": "reply",
            "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
        }]
    }"#;
    {
        let mut state = github.state();
        for (path, text) in [
            (
                "factory.json",
                r#"{ "version": 1, "flows": ["notes"] }"#.to_string(),
            ),
            ("flows/notes.json", flow.to_string()),
            ("agents.json", agents.to_string()),
        ] {
            state
                .files
                .insert((FIRST.to_string(), path.to_string()), text);
        }
    }
    github
}

/// A cancel that was taken keeps who sent it through a record that fails
/// for a moment: the record is tried again, since a caller's retry would
/// find the Run cancelled and keep no one. A record that keeps failing
/// answers an error.
#[tokio::test(start_paused = true)]
async fn a_taken_cancel_keeps_its_sender_through_a_failing_record() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let run_id = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;
    let backup = Some(ActingAgent::Bot(BotId::new_from_uuid(BACKUP_BOT)));
    repo.failing
        .lock()
        .expect("failing")
        .insert("record_standing_act", (0, 2));

    service
        .cancel_run(&julia(), run_id, backup, false)
        .await
        .expect("the operator cancels");

    let detail = service.get_run(&julia(), run_id).await.expect("read");
    let kept: Vec<_> = detail
        .standing_acts
        .iter()
        .map(|act| (act.act, act.bot_id))
        .collect();
    assert_eq!(kept, [(StandingAct::Cancel, BACKUP_BOT)]);

    let other = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;
    repo.failing
        .lock()
        .expect("failing")
        .insert("record_standing_act", (0, 3));
    let lost = service.cancel_run(&julia(), other, backup, false).await;
    assert!(lost.is_err(), "{lost:?}");
}

/// Starting a Run of a Definition with Standing Roles takes a bot that
/// fills one allowing `start`, and the Run keeps who started it; the
/// owner in person starts it as before.
#[tokio::test]
async fn starting_a_run_takes_a_bot_in_a_standing_role_that_allows_it() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = notes_definition();
    let service = code_service(&repo, &workflows, &github);
    let primary = Uuid::now_v7();
    let backup = Uuid::now_v7();
    repo.add_session_bot(primary, teo().as_ref(), PRIMARY_BOT);
    repo.add_session_bot(backup, teo().as_ref(), BACKUP_BOT);
    let start = |acting: Option<ActingAgent>| {
        service.create_run(
            teo(),
            "notes".to_string(),
            "Note it".to_string(),
            None,
            None,
            acting.is_none(),
            acting,
        )
    };

    let refused = start(Some(ActingAgent::Session(primary))).await;
    let started = start(Some(ActingAgent::Session(backup)))
        .await
        .expect("the intake starts");
    let in_person = start(None).await.expect("the owner starts in person");

    let Err(FactoryError::NotInStandingRole { act, allowing }) = refused else {
        panic!("a bot in no standing role that starts is refused");
    };
    assert_eq!(
        (act, allowing),
        (StandingAct::Start, vec![standing_role("intake")])
    );
    assert_eq!(repo.runs.lock().expect("runs").len(), 2);
    assert_eq!(
        started
            .run
            .definition
            .as_ref()
            .map(|kept| kept.repo_url.as_str()),
        Some(DEFINITION_REPO)
    );
    let kept: Vec<_> = started
        .standing_acts
        .iter()
        .map(|act| {
            (
                act.act,
                act.standing_role.as_str().to_string(),
                act.bot_id,
                act.session_id,
            )
        })
        .collect();
    assert_eq!(
        kept,
        [(
            StandingAct::Start,
            "intake".to_string(),
            BACKUP_BOT,
            Some(backup)
        )]
    );
    assert!(in_person.standing_acts.is_empty());
}

/// A bot in no agent session starting a Run is checked as that bot: one in
/// no Standing Role that allows `start` is refused, and the Run another
/// starts keeps its bot with no session.
#[tokio::test]
async fn a_sessionless_bot_starts_a_run_only_through_a_standing_role() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = notes_definition();
    let service = code_service(&repo, &workflows, &github);
    let start = |bot: Uuid| {
        service.create_run(
            teo(),
            "notes".to_string(),
            "Note it".to_string(),
            None,
            None,
            false,
            Some(ActingAgent::Bot(BotId::new_from_uuid(bot))),
        )
    };

    let refused = start(PRIMARY_BOT).await;
    let started = start(BACKUP_BOT).await.expect("the intake starts");

    let Err(FactoryError::NotInStandingRole { act, allowing }) = refused else {
        panic!("a sessionless bot in no standing role that starts is refused");
    };
    assert_eq!(
        (act, allowing),
        (StandingAct::Start, vec![standing_role("intake")])
    );
    assert_eq!(repo.runs.lock().expect("runs").len(), 1);
    let kept: Vec<_> = started
        .standing_acts
        .iter()
        .map(|act| {
            (
                act.act,
                act.standing_role.as_str().to_string(),
                act.bot_id,
                act.session_id,
            )
        })
        .collect();
    assert_eq!(
        kept,
        [(StandingAct::Start, "intake".to_string(), BACKUP_BOT, None)]
    );
}

/// Code signed for a bot that runs in no agent session is checked as that
/// bot: under Standing Roles it cancels and links only through one that
/// allows the act, and the Run keeps the bot with no session.
#[tokio::test]
async fn a_sessionless_bot_takes_a_standing_role_that_allows_the_act() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let run_id = waiting_run(
        &repo,
        Some(agents(Some(split_roles()))),
        "plan",
        "done",
        &["approve", "revise"],
    )
    .await;
    let primary = Some(ActingAgent::Bot(BotId::new_from_uuid(PRIMARY_BOT)));
    let backup = Some(ActingAgent::Bot(BotId::new_from_uuid(BACKUP_BOT)));
    let unknown = Some(ActingAgent::Bot(BotId::new_from_uuid(Uuid::now_v7())));

    let cancel_refused = service.cancel_run(&julia(), run_id, primary, false).await;
    let link_refused = service.link_task(&julia(), run_id, task, unknown).await;
    let linked = service.link_task(&julia(), run_id, task, backup).await;
    let cancelled = service.cancel_run(&julia(), run_id, backup, false).await;

    let Err(FactoryError::NotInStandingRole { act, allowing }) = cancel_refused else {
        panic!("a sessionless bot in no standing role that cancels is refused");
    };
    assert_eq!(
        (act, allowing),
        (StandingAct::Cancel, vec![standing_role("operator")])
    );
    assert!(
        matches!(
            link_refused,
            Err(FactoryError::NotInStandingRole {
                act: StandingAct::Link,
                ..
            })
        ),
        "{link_refused:?}"
    );
    linked.expect("the intake links");
    cancelled.expect("the operator cancels");
    assert_eq!(*workflows.cancelled.lock().expect("cancels"), [run_id]);
    let detail = service.get_run(&julia(), run_id).await.expect("read");
    let kept: Vec<_> = detail
        .standing_acts
        .iter()
        .map(|act| (act.act, act.bot_id, act.session_id))
        .collect();
    assert_eq!(
        kept,
        [
            (StandingAct::Link, BACKUP_BOT, None),
            (StandingAct::Cancel, BACKUP_BOT, None),
        ]
    );
}

/// The owner in person cancels and links a Run whatever Agents it kept:
/// they are read only for an agent's act, so Agents that no longer read
/// stop no one but an agent.
#[tokio::test]
async fn the_owner_in_person_acts_whatever_agents_the_run_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let run_id = waiting_run(
        &repo,
        Some(json!({ "agents": "no longer reads" })),
        "plan",
        "done",
        &["approve"],
    )
    .await;

    service
        .link_task(&julia(), run_id, task, None)
        .await
        .expect("the owner links in person");
    service
        .cancel_run(&julia(), run_id, None, true)
        .await
        .expect("the owner cancels in person");
    let agent = service
        .cancel_run(
            &julia(),
            run_id,
            Some(ActingAgent::Bot(BotId::new_from_uuid(PRIMARY_BOT))),
            false,
        )
        .await;

    assert!(matches!(agent, Err(FactoryError::Internal(_))), "{agent:?}");
}

/// A Benchmark suite starts a trial Run of each task as its caller: an
/// agent starting one under a Definition with Standing Roles takes a bot
/// that fills one allowing `start`, checked once, before anything is kept,
/// and each trial keeps who started it. A bot in no such Standing Role is
/// refused as `POST /runs` refuses it, and nothing is kept.
#[tokio::test]
async fn a_suite_takes_a_bot_in_a_standing_role_that_allows_start() {
    let repo = team_repo();
    let workflows = FakeWorkflows::default();
    let github = notes_definition();
    {
        let mut state = github.state();
        let agents = json!({
            "agents": {
                "primary": { "bot": PRIMARY_BOT.to_string() },
                "backup": { "bot": BACKUP_BOT.to_string() }
            },
            "roles": { "planner": ["primary"], "writer": ["primary"] },
            "standing": split_roles()
        });
        state.files.insert(
            (FIRST.to_string(), "agents.json".to_string()),
            agents.to_string(),
        );
        state.files.insert(
            (FIRST.to_string(), "scorers.json".to_string()),
            r#"{
                "benchmarks": [{
                    "name": "core", "repository": "https://github.com/macro/widgets",
                    "tasks": [
                        { "name": "typo", "request": "Fix the typo." },
                        { "name": "export", "request": "Add an export." }
                    ]
                }]
            }"#
            .to_string(),
        );
    }
    let suites = EvaluationServiceImpl::new(
        repo.clone(),
        Arc::new(code_service(&repo, &workflows, &github)),
        Arc::new(EvaluationDefinition::default()),
        None,
    );
    let writer: AgentChoices = [(
        Role::try_from("writer".to_string()).expect("valid"),
        AgentName::try_from("primary".to_string()).expect("valid"),
    )]
    .into_iter()
    .collect();
    let primary = Uuid::now_v7();
    let backup = Uuid::now_v7();
    repo.add_session_bot(primary, teo().as_ref(), PRIMARY_BOT);
    repo.add_session_bot(backup, teo().as_ref(), BACKUP_BOT);

    let refused = suites
        .start_suite(
            teo(),
            "core",
            writer.clone(),
            Some(ActingAgent::Session(primary)),
        )
        .await;
    let Err(SuiteError::Run(FactoryError::NotInStandingRole { act, allowing })) = refused else {
        panic!("a bot in no standing role that starts is refused: {refused:?}");
    };
    assert_eq!(
        (act, allowing),
        (StandingAct::Start, vec![standing_role("intake")])
    );
    assert!(repo.runs.lock().expect("runs").is_empty());
    assert!(
        repo.evaluation
            .lock()
            .expect("evaluation")
            .suites
            .is_empty()
    );
    assert!(workflows.started.lock().expect("started").is_empty());

    let started = suites
        .start_suite(teo(), "core", writer, Some(ActingAgent::Session(backup)))
        .await
        .expect("the intake starts a suite");

    assert_eq!(started.runs.len(), 2);
    let acts = repo.standing_acts.lock().expect("standing acts").clone();
    for run in started.runs.values() {
        let kept: Vec<_> = acts
            .iter()
            .filter(|(id, _)| id == run)
            .map(|(_, act)| {
                (
                    act.act,
                    act.standing_role.as_str().to_string(),
                    act.bot_id,
                    act.session_id,
                )
            })
            .collect();
        assert_eq!(
            kept,
            [(
                StandingAct::Start,
                "intake".to_string(),
                BACKUP_BOT,
                Some(backup)
            )]
        );
    }
}
