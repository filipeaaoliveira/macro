//! A Run's token budget: kept from its rules when it starts, checked by the
//! worker before each agent Step, told to its owner once it ends the Run,
//! and shown by the Run API with what the Run used of it.

use factory_records::domain::models::{
    BudgetStop, NewRun, RunId, RunInsert, RunOutcome, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules};
use uuid::Uuid;

use super::{FakeExecutor, FakeWorkflows, code_service, julia};
use crate::domain::fakes::{FakeGithub, FakeNotifier, FakeRepo, FakeRunTokens, FakeTaskNotes};
use crate::domain::models::{FactoryError, RunFinish};
use crate::domain::ports::{FactoryService, StepService};
use crate::domain::service::StepServiceImpl;
use crate::domain::token_budget::{BudgetCheck, BudgetVerdict, MAX_UNREAD_CHECKS, notice_id};

const BUDGET: u64 = 100_000;

/// The factory's default rules with a token budget of `budget`, whose Gates
/// notify no one when `quiet`.
fn budgeted(budget: Option<u64>, quiet: bool) -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            token_budget: budget,
            quiet_gates: quiet,
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// A `plan` Run of Julia's on a task, with `rules`.
async fn run_with(repo: &FakeRepo, rules: RunRules) -> RunId {
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Plan the dark mode",
            user_id: &julia(),
            task_id: Some(task),
            project_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run on a task no other run works on is kept");
    };
    run.id
}

/// The worker, reading the usage log `usage` and telling owners through
/// `notifier`.
fn worker(
    repo: &FakeRepo,
    usage: &FakeRunTokens,
    notifier: &FakeNotifier,
) -> StepServiceImpl<FakeRepo, FakeExecutor, FakeNotifier, FakeTaskNotes> {
    StepServiceImpl::new(repo.clone(), FakeExecutor::default())
        .with_owner_gates(notifier.clone(), FakeTaskNotes::default())
        .with_usage(usage.clone())
}

fn check(run_id: RunId, unread: u8) -> BudgetCheck {
    BudgetCheck { run_id, unread }
}

async fn stop_of(repo: &FakeRepo, run_id: RunId) -> Option<BudgetStop> {
    repo.get_run(run_id)
        .await
        .expect("read")
        .expect("kept")
        .budget_stop
}

/// Julia administers a team whose rules are `definition`.
fn julia_s_team(repo: &FakeRepo, definition: OrchestratorDefinition) -> Uuid {
    let team_id = Uuid::now_v7();
    repo.teams.lock().expect("teams").insert(
        julia().to_string(),
        TeamMembership {
            team_id,
            admin: true,
        },
    );
    repo.team_rules
        .lock()
        .expect("rules")
        .insert(team_id, definition);
    team_id
}

/// A Run under its budget goes on; one that used it, or more, keeps how
/// much and is stopped, and a retry of that check finds the stop kept
/// whatever the log reads now.
#[tokio::test]
async fn a_run_that_used_its_budget_is_stopped_and_keeps_why() {
    let repo = FakeRepo::default();
    let run_id = run_with(&repo, budgeted(Some(BUDGET), false)).await;
    let usage = FakeRunTokens::reading([Some(BUDGET - 1), Some(BUDGET), Some(0)]);
    let worker = worker(&repo, &usage, &FakeNotifier::default());

    let first = worker.check_budget(check(run_id, 0)).await.expect("check");
    assert_eq!(first, BudgetVerdict::Within);
    assert_eq!(stop_of(&repo, run_id).await, None);

    let second = worker.check_budget(check(run_id, 0)).await.expect("check");
    assert_eq!(second, BudgetVerdict::Stopped);
    assert_eq!(
        stop_of(&repo, run_id).await,
        Some(BudgetStop::Spent { used: BUDGET })
    );

    let retried = worker.check_budget(check(run_id, 0)).await.expect("check");
    assert_eq!(retried, BudgetVerdict::Stopped);
    assert_eq!(
        stop_of(&repo, run_id).await,
        Some(BudgetStop::Spent { used: BUDGET })
    );
}

/// A usage log that cannot be read lets the Step go on, unchecked, for at
/// most `MAX_UNREAD_CHECKS - 1` checks in a row: the next ends the Run,
/// as unread. A read in between starts the count again, which the
/// workflow keeps.
#[tokio::test]
async fn an_unread_usage_log_lets_the_run_go_on_a_bounded_while() {
    let repo = FakeRepo::default();
    let run_id = run_with(&repo, budgeted(Some(BUDGET), false)).await;
    let usage = FakeRunTokens::reading([None]);
    let worker = worker(&repo, &usage, &FakeNotifier::default());

    for unread in 0..MAX_UNREAD_CHECKS - 1 {
        let verdict = worker
            .check_budget(check(run_id, unread))
            .await
            .expect("an unread log is no failure");
        assert_eq!(verdict, BudgetVerdict::Unread, "after {unread} unread");
        assert_eq!(stop_of(&repo, run_id).await, None);
    }
    let last = worker
        .check_budget(check(run_id, MAX_UNREAD_CHECKS - 1))
        .await
        .expect("check");
    assert_eq!(last, BudgetVerdict::Stopped);
    assert_eq!(stop_of(&repo, run_id).await, Some(BudgetStop::Unread));
}

/// A Run with no budget is never stopped, and its check reads no usage.
#[tokio::test]
async fn a_run_without_a_budget_is_within_and_reads_no_usage() {
    let repo = FakeRepo::default();
    let run_id = run_with(&repo, budgeted(None, false)).await;
    let usage = FakeRunTokens::reading([Some(u64::MAX)]);
    let worker = worker(&repo, &usage, &FakeNotifier::default());

    let verdict = worker.check_budget(check(run_id, 0)).await.expect("check");
    assert_eq!(verdict, BudgetVerdict::Within);
    assert_eq!(usage.reads(), 0);
    assert_eq!(stop_of(&repo, run_id).await, None);
}

/// Once a Run its budget stopped ends `blocked`, its owner is told why,
/// with one notice however often the end is recorded; a quiet Run tells no
/// one, and a Run that ended `blocked` for its Steps sends no budget
/// notice.
#[tokio::test]
async fn the_owner_is_told_once_their_run_ends_at_its_budget() {
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    let usage = FakeRunTokens::reading([Some(BUDGET + 5)]);
    let worker = worker(&repo, &usage, &notifier);
    let told = run_with(&repo, budgeted(Some(BUDGET), false)).await;
    let quiet = run_with(&repo, budgeted(Some(BUDGET), true)).await;
    let unstopped = run_with(&repo, budgeted(Some(BUDGET), false)).await;

    for run_id in [told, quiet] {
        worker.check_budget(check(run_id, 0)).await.expect("check");
    }
    for run_id in [told, told, quiet, unstopped] {
        worker
            .end_run(RunFinish {
                run_id,
                outcome: RunOutcome::Blocked,
            })
            .await
            .expect("end");
    }

    let sent = notifier.sent.lock().expect("sent").clone();
    assert_eq!(sent.len(), 2, "one notice, sent again on a retry: {sent:?}");
    for notice in &sent {
        assert_eq!(notice.notification_id, notice_id(told));
        assert_eq!(notice.metadata.run_id, told.as_uuid());
        assert!(notice.push);
        assert!(!notice.metadata.notify_only);
        assert!(notice.metadata.budget_stopped);
        assert!(
            notice
                .metadata
                .question
                .contains(&format!("used {} tokens of {BUDGET}", BUDGET + 5)),
            "{}",
            notice.metadata.question
        );
    }
}

/// A Run started under rules with a budget keeps it, so a change to the
/// rules later does not change it, and its workflow is told to check it.
/// Read on its own, it shows what its agent Steps used; listed, only its
/// budget. A Run started under rules with none is not checked and shows
/// no budget.
#[tokio::test]
async fn a_run_keeps_its_budget_and_the_run_api_shows_its_use() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let usage = FakeRunTokens::reading([Some(42_000)]);
    let service = code_service(&repo, &workflows, &FakeGithub::default()).with_usage(usage.clone());
    let budget = OrchestratorDefinition {
        token_budget: Some(BUDGET),
        ..OrchestratorDefinition::DEFAULT
    };
    julia_s_team(&repo, budget);
    let start = || {
        service.create_run(
            julia(),
            "plan".to_string(),
            "Plan it".to_string(),
            None,
            None,
            true,
            None,
        )
    };

    let started = start().await.expect("a run starts").run.id;
    service
        .set_standing_rules(julia(), Some(OrchestratorDefinition::DEFAULT))
        .await
        .expect("the team's rules change");
    let unbudgeted = start().await.expect("another starts").run.id;

    let starts = workflows.started.lock().expect("started").clone();
    assert_eq!(
        starts
            .iter()
            .map(|start| (start.run_id, start.token_budget))
            .collect::<Vec<_>>(),
        [(started, true), (unbudgeted, false)]
    );

    let read = service.get_run(&julia(), started).await.expect("read");
    let shown = read.token_budget.expect("a budget");
    assert_eq!(shown.budget, BUDGET);
    assert_eq!(shown.used, Some(42_000));
    assert_eq!(shown.stop, None);
    let json = serde_json::to_value(&read).expect("serialize");
    assert_eq!(json["tokenBudget"]["budget"], BUDGET);
    assert_eq!(json["tokenBudget"]["used"], 42_000);

    let listed = service.list_starting_runs(&julia()).await.expect("listed");
    let listed = listed
        .iter()
        .find(|detail| detail.run.id == started)
        .expect("listed");
    assert_eq!(
        listed.token_budget.map(|shown| (shown.budget, shown.used)),
        Some((BUDGET, None))
    );

    let reads = usage.reads();
    let read = service.get_run(&julia(), unbudgeted).await.expect("read");
    assert_eq!(read.token_budget, None);
    assert_eq!(usage.reads(), reads, "a run with no budget reads no usage");
}

/// A Run whose usage cannot be read still reads, its use unknown, and
/// shows why its budget stopped it.
#[tokio::test]
async fn a_run_reads_with_its_use_unknown_and_its_stop() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let usage = FakeRunTokens::reading([None]);
    let service = code_service(&repo, &workflows, &FakeGithub::default()).with_usage(usage);
    let run_id = run_with(&repo, budgeted(Some(BUDGET), false)).await;
    repo.record_budget_stop(run_id, BudgetStop::Unread)
        .await
        .expect("stop");

    let read = service.get_run(&julia(), run_id).await.expect("read");
    let shown = read.token_budget.expect("a budget");
    assert_eq!(shown.used, None);
    assert_eq!(shown.stop, Some(BudgetStop::Unread));
    let json = serde_json::to_value(&read).expect("serialize");
    assert_eq!(json["tokenBudget"]["stop"]["reason"], "unread");
}

/// A token budget out of range is refused before anything is kept, for a
/// team and a Project; the smallest and largest are kept.
#[tokio::test]
async fn a_token_budget_out_of_range_is_refused() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let team_id = julia_s_team(&repo, OrchestratorDefinition::DEFAULT);
    let with_budget = |budget: u64| {
        Some(OrchestratorDefinition {
            token_budget: Some(budget),
            ..OrchestratorDefinition::DEFAULT
        })
    };

    for budget in [0, 9_999, 10_000_000_001] {
        let refused = service
            .set_standing_rules(julia(), with_budget(budget))
            .await;
        assert!(
            matches!(
                refused,
                Err(FactoryError::InvalidTokenBudget {
                    min: 10_000,
                    max: 10_000_000_000
                })
            ),
            "{budget}: {refused:?}"
        );
        let refused = service
            .set_project_rules(julia(), Uuid::now_v7(), with_budget(budget))
            .await;
        assert!(
            matches!(refused, Err(FactoryError::InvalidTokenBudget { .. })),
            "{budget}: {refused:?}"
        );
    }
    assert_eq!(
        repo.team_rules.lock().expect("rules").get(&team_id),
        Some(&OrchestratorDefinition::DEFAULT)
    );

    for budget in [10_000, 10_000_000_000] {
        service
            .set_standing_rules(julia(), with_budget(budget))
            .await
            .expect("a budget in range is kept");
        assert_eq!(
            repo.team_rules
                .lock()
                .expect("rules")
                .get(&team_id)
                .and_then(|rules| rules.token_budget),
            Some(budget)
        );
    }
}

/// A Run whose agent sessions recorded no usage at all (an agent whose
/// runtime is not measured) reads as not measured, its use 0, and its
/// budget never stops it.
#[tokio::test]
async fn a_run_whose_agents_record_no_usage_reads_as_not_measured() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let usage = FakeRunTokens::unmeasured();
    let service = code_service(&repo, &workflows, &FakeGithub::default()).with_usage(usage.clone());
    let run_id = run_with(&repo, budgeted(Some(BUDGET), false)).await;

    let read = service.get_run(&julia(), run_id).await.expect("read");
    let shown = read.token_budget.expect("a budget");
    assert_eq!((shown.used, shown.measured), (Some(0), false));
    let json = serde_json::to_value(&read).expect("serialize");
    assert_eq!(json["tokenBudget"]["measured"], false);

    let worker = worker(&repo, &usage, &FakeNotifier::default());
    for unread in 0..MAX_UNREAD_CHECKS {
        let verdict = worker
            .check_budget(check(run_id, unread))
            .await
            .expect("check");
        assert_eq!(verdict, BudgetVerdict::Within);
    }
    assert_eq!(stop_of(&repo, run_id).await, None);

    // A measured Run reads so.
    let measured = code_service(&repo, &workflows, &FakeGithub::default())
        .with_usage(FakeRunTokens::reading([Some(7)]));
    let shown = measured
        .get_run(&julia(), run_id)
        .await
        .expect("read")
        .token_budget
        .expect("a budget");
    assert_eq!((shown.used, shown.measured), (Some(7), true));
}
