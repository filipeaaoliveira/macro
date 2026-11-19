use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use super::super::test::{insert, julia};
use crate::domain::evaluation::{EvaluationRepository, Judgment};
use crate::domain::models::{RunId, RunOutcome};
use crate::domain::ports::RunRepository;
use crate::domain::self_improvement::{
    FailureSource, ImprovementBegin, ImprovementState, NewFailure, SelfImprovementRepository,
    Settlement,
};

const REPO_URL: &str = "https://github.com/macro/factory-definition";

fn team() -> Uuid {
    Uuid::parse_str("00000000-0000-7000-8000-000000000201").expect("valid")
}

fn scorer_failure(name: &str, score: f64) -> NewFailure {
    NewFailure {
        source: FailureSource::Scorer {
            name: name.to_string(),
            score,
        },
        reason: format!("{name} found it wrong"),
    }
}

fn flawed() -> NewFailure {
    NewFailure {
        source: FailureSource::Flawed,
        reason: "The button is the wrong colour".to_string(),
    }
}

/// A Run of Julia's with `failures` kept.
async fn failing_run(repo: &PgRunRepository, failures: &[NewFailure]) -> RunId {
    let id = RunId::generate();
    insert(repo, id, "Add dark mode").await;
    repo.record_failures(team(), REPO_URL, id, failures)
        .await
        .expect("record");
    id
}

/// The namer of a team's Definition is kept with it, and replaced when an
/// admin names it again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_definition_keeps_who_named_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    assert_eq!(repo.definition_naming(team()).await.expect("read"), None);

    repo.set_team_definition(team(), Some(REPO_URL), &julia())
        .await
        .expect("name");
    let naming = repo
        .definition_naming(team())
        .await
        .expect("read")
        .expect("named");
    assert_eq!(naming.repo_url, REPO_URL);
    assert_eq!(naming.named_by, Some(julia()));
}

/// Failures are kept once per Run and source, counted by source while
/// pending, and only under the Definition they were kept for.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn pending_failures_are_counted_once_each(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let first = failing_run(&repo, &[scorer_failure("tests", 0.2), flawed()]).await;
    // Kept again, as a retried activity would: nothing new.
    repo.record_failures(team(), REPO_URL, first, &[scorer_failure("tests", 0.1)])
        .await
        .expect("again");
    failing_run(&repo, &[scorer_failure("tests", 0.3)]).await;

    let pending = repo
        .pending_failures(team(), REPO_URL)
        .await
        .expect("pending");
    assert_eq!(pending.count, 3);
    assert!(pending.oldest.is_some());
    assert_eq!(
        pending.by_source,
        vec![("tests".to_string(), 2), ("flawed".to_string(), 1)]
    );

    let elsewhere = repo
        .pending_failures(team(), "https://github.com/macro/other")
        .await
        .expect("pending");
    assert_eq!(elsewhere.count, 0);
}

/// A team holds at most one open self-improvement Run: the first begin
/// claims the oldest pending failures up to its limit, a second finds it
/// open, and one with nothing pending keeps nothing.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn one_improvement_is_open_per_team(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let empty = repo
        .begin_improvement(Uuid::now_v7(), team(), REPO_URL, 10)
        .await
        .expect("begin");
    assert_eq!(empty, ImprovementBegin::NoFailures);
    assert_eq!(repo.latest_improvement(team()).await.expect("read"), None);

    let first = failing_run(&repo, &[scorer_failure("tests", 0.2)]).await;
    failing_run(&repo, &[flawed()]).await;

    let id = Uuid::now_v7();
    let ImprovementBegin::Claimed(claimed) = repo
        .begin_improvement(id, team(), REPO_URL, 1)
        .await
        .expect("begin")
    else {
        panic!("the oldest failure is claimed");
    };
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].run_id, first);
    assert_eq!(claimed[0].flow, "plan");
    assert_eq!(
        repo.pending_failures(team(), REPO_URL)
            .await
            .expect("pending")
            .count,
        1
    );

    let second = repo
        .begin_improvement(Uuid::now_v7(), team(), REPO_URL, 10)
        .await
        .expect("begin");
    assert_eq!(second, ImprovementBegin::AlreadyOpen);

    let latest = repo
        .latest_improvement(team())
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(latest.id, id);
    assert_eq!(latest.state, ImprovementState::Starting);
    assert_eq!(latest.failures, 1);
}

/// A merged self-improvement Run addresses its failures; a closed one lets
/// them go back to pending, so the next one claims them again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn settling_addresses_or_releases_failures(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    failing_run(&repo, &[scorer_failure("tests", 0.2)]).await;

    let closed = Uuid::now_v7();
    repo.begin_improvement(closed, team(), REPO_URL, 10)
        .await
        .expect("begin");
    let run = RunId::generate();
    insert(&repo, run, "Improve the definition").await;
    assert!(repo.link_improvement_run(closed, run).await.expect("link"));
    assert!(!repo.link_improvement_run(closed, run).await.expect("again"));
    repo.settle_improvement(closed, &Settlement::Closed)
        .await
        .expect("close");
    let latest = repo
        .latest_improvement(team())
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(latest.state, ImprovementState::Closed);
    assert_eq!(latest.run_id, Some(run));
    assert!(latest.settled_at.is_some());
    assert_eq!(
        repo.pending_failures(team(), REPO_URL)
            .await
            .expect("pending")
            .count,
        1
    );

    let merged = Uuid::now_v7();
    let ImprovementBegin::Claimed(claimed) = repo
        .begin_improvement(merged, team(), REPO_URL, 10)
        .await
        .expect("begin")
    else {
        panic!("the released failure is claimed again");
    };
    assert_eq!(claimed.len(), 1);
    repo.settle_improvement(merged, &Settlement::Merged)
        .await
        .expect("merge");
    // Settling again changes nothing.
    repo.settle_improvement(merged, &Settlement::Closed)
        .await
        .expect("again");
    assert_eq!(
        repo.latest_improvement(team())
            .await
            .expect("read")
            .expect("kept")
            .state,
        ImprovementState::Merged
    );
    assert_eq!(
        repo.pending_failures(team(), REPO_URL)
            .await
            .expect("pending")
            .count,
        0
    );
    assert_eq!(
        repo.begin_improvement(Uuid::now_v7(), team(), REPO_URL, 10)
            .await
            .expect("begin"),
        ImprovementBegin::NoFailures
    );
}

/// A refused Run keeps why, and lets its failures go back.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_refused_improvement_keeps_why(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    failing_run(&repo, &[flawed()]).await;
    let id = Uuid::now_v7();
    repo.begin_improvement(id, team(), REPO_URL, 10)
        .await
        .expect("begin");
    repo.settle_improvement(id, &Settlement::Refused("no push access".to_string()))
        .await
        .expect("refuse");
    let latest = repo
        .latest_improvement(team())
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(latest.state, ImprovementState::Refused);
    assert_eq!(latest.refusal.as_deref(), Some("no push access"));
    assert_eq!(latest.failures, 1);
    assert_eq!(
        repo.pending_failures(team(), REPO_URL)
            .await
            .expect("pending")
            .count,
        1
    );
}

/// What a Run that ended tells: its owner, Flow, Definition, outcome and
/// the live Scorers' judgments of it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_runs_failure_facts_read_its_judgments(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Add dark mode").await;
    repo.end_run(id, RunOutcome::Success).await.expect("end");
    repo.record_run_score(
        id,
        &Judgment {
            scorer: "tests".to_string(),
            score: 0.25,
            reason: "No test covers it".to_string(),
            model: "judge".to_string(),
        },
    )
    .await
    .expect("score");

    let facts = repo.failure_facts(id).await.expect("read").expect("found");
    assert_eq!(facts.user_id, julia());
    assert_eq!(facts.flow, "plan");
    assert_eq!(facts.outcome, Some(RunOutcome::Success));
    assert_eq!(facts.definition_repo_url, None);
    assert_eq!(
        facts.judgments,
        vec![("tests".to_string(), 0.25, "No test covers it".to_string())]
    );
    assert_eq!(
        repo.failure_facts(RunId::generate()).await.expect("read"),
        None
    );
}

/// Pending failures are matched to the Definition's repository ignoring
/// case, as a team naming it again in other letters would: counted and
/// claimed alike.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn pending_failures_match_the_repository_in_any_case(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    failing_run(&repo, &[scorer_failure("tests", 0.2), flawed()]).await;
    let upper = REPO_URL.replace("macro/factory-definition", "Macro/Factory-Definition");

    let pending = repo
        .pending_failures(team(), &upper)
        .await
        .expect("pending");
    assert_eq!(pending.count, 2);
    let begun = repo
        .begin_improvement(Uuid::now_v7(), team(), &upper, 50)
        .await
        .expect("begin");
    assert!(
        matches!(&begun, ImprovementBegin::Claimed(failures) if failures.len() == 2),
        "{begun:?}"
    );
}

/// A team's latest self-improvement Runs are read newest first, at most
/// the number asked for, and only the team's.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn recent_improvements_read_newest_first(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let mut ids = Vec::new();
    for settlement in [
        Settlement::Merged,
        Settlement::Closed,
        Settlement::Refused("the run could not start".to_string()),
    ] {
        failing_run(&repo, &[flawed()]).await;
        let id = Uuid::now_v7();
        let begun = repo
            .begin_improvement(id, team(), REPO_URL, 50)
            .await
            .expect("begin");
        assert!(matches!(begun, ImprovementBegin::Claimed(_)), "{begun:?}");
        repo.settle_improvement(id, &settlement)
            .await
            .expect("settle");
        ids.push(id);
    }

    let recent = repo.recent_improvements(team(), 2).await.expect("recent");
    let states: Vec<_> = recent
        .iter()
        .map(|improvement| (improvement.id, improvement.state))
        .collect();
    assert_eq!(
        states,
        vec![
            (ids[2], ImprovementState::Refused),
            (ids[1], ImprovementState::Closed),
        ]
    );
    assert!(
        recent
            .iter()
            .all(|improvement| improvement.settled_at.is_some())
    );
    assert!(
        repo.recent_improvements(Uuid::now_v7(), 8)
            .await
            .expect("recent")
            .is_empty()
    );
}

/// Naming another Definition, or none, settles the team's open
/// self-improvement Run as closed in the same write, its failures back to
/// pending; naming the same one in another case settles nothing.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn naming_another_definition_closes_the_open_improvement(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    repo.set_team_definition(team(), Some(REPO_URL), &julia())
        .await
        .expect("name");
    failing_run(&repo, &[scorer_failure("tests", 0.2)]).await;
    let id = Uuid::now_v7();
    let begun = repo
        .begin_improvement(id, team(), REPO_URL, 10)
        .await
        .expect("begin");
    assert!(matches!(begun, ImprovementBegin::Claimed(_)));

    repo.set_team_definition(
        team(),
        Some("https://github.com/Macro/Factory-Definition"),
        &julia(),
    )
    .await
    .expect("same, in another case");
    let latest = repo
        .latest_improvement(team())
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(latest.state, ImprovementState::Starting);

    repo.set_team_definition(team(), None, &julia())
        .await
        .expect("un-name");
    let latest = repo
        .latest_improvement(team())
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(latest.state, ImprovementState::Closed);
    assert!(latest.settled_at.is_some());
    assert_eq!(
        repo.pending_failures(team(), REPO_URL)
            .await
            .expect("pending")
            .count,
        1
    );
}
