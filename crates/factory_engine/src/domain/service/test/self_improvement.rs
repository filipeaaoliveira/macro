//! Self-improvement: a team's Scorer failures and flawed Runs under its
//! Definition start one Run against the Definition, whose merge only a
//! person gives, and which settles them.

use std::sync::Arc;

use chrono::{TimeDelta, Utc};
use factory_records::domain::models::{
    EntryId, Outcome, RunId, RunOutcome, StepKey, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::self_improvement::{
    FailureSource, IMPROVE_FLOW, ImprovementState, NewFailure, SelfImprovementRepository,
};
use factory_records::domain::standing_rules::RuleGateKind;
use uuid::Uuid;

use super::{FakeWorkflows, code_service, julia, teo};
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::evaluation::EvaluationDefinition;
use crate::domain::evaluation::service::ScoringServiceImpl;
use crate::domain::fakes::FakeWatches;
use crate::domain::fakes::{FakeGithub, FakeJudge, FakeRepo};
use crate::domain::flow::StepName;
use crate::domain::insights::fakes::FakeUsage;
use crate::domain::models::StepRequest;
use crate::domain::models::{FactoryError, PullRequest, PullRequestState};
use crate::domain::ports::StepService;
use crate::domain::ports::{FactoryService, ScoringService};
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::self_improvement::{
    ImprovingScoring, SelfImprovement, SelfImprovementServiceImpl,
};
use crate::domain::service::StepServiceImpl;
use crate::domain::standing_rules::{GateFacts, rule_answer};
use crate::outbound::local_agent::LocalAgent;

const TEAM: Uuid = Uuid::from_u128(0x5e1f);

const DEFINITION_REPO: &str = "https://github.com/macro/factory-definition";

const HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";

const APP: &str = "https://app.example";

/// Agents that fill the notes Flow and the improve Flow.
const AGENTS: &str = r#"{
    "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
    "roles": {
        "writer": ["primary"], "planner": ["primary"], "reviewer": ["primary"],
        "fixer": ["primary"]
    }
}"#;

/// A Definition of one Flow, `notes`, that replies, with a Scorer that
/// judges every notes Run and starts a self-improvement Run at two
/// failures, and `rules`.
fn definition(github: &FakeGithub, agents: &str, rules: Option<&str>) {
    let flow = r#"{
        "description": "Notes.",
        "entries": [{
            "id": "reply", "step": "reply",
            "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
        }]
    }"#;
    let scorers = r#"{
        "scorers": [{
            "name": "reads-well", "instructions": "Do the notes read well?",
            "flows": ["notes"], "samplePercent": 100
        }],
        "selfImprovement": { "failures": 2, "oldestDays": 7 }
    }"#;
    let mut files = vec![
        ("factory.json", r#"{ "version": 1, "flows": ["notes"] }"#),
        ("flows/notes.json", flow),
        ("agents.json", agents),
        ("scorers.json", scorers),
    ];
    if let Some(rules) = rules {
        files.push(("rules.json", rules));
    }
    let mut state = github.state();
    state.head = Some(HEAD.to_string());
    for (path, text) in files {
        state
            .files
            .insert((HEAD.to_string(), path.to_string()), text.to_string());
    }
}

/// Julia administers the team and named its Definition; Teo is on it.
fn team_repo(named: bool) -> FakeRepo {
    let repo = FakeRepo::default();
    let mut teams = repo.teams.lock().expect("teams");
    teams.insert(
        julia().to_string(),
        TeamMembership {
            team_id: TEAM,
            admin: true,
        },
    );
    teams.insert(
        teo().to_string(),
        TeamMembership {
            team_id: TEAM,
            admin: false,
        },
    );
    drop(teams);
    repo.team_definitions
        .lock()
        .expect("team definitions")
        .insert(TEAM, DEFINITION_REPO.to_string());
    if named {
        repo.definition_namers
            .lock()
            .expect("namers")
            .insert(TEAM, julia().to_string());
    }
    repo
}

/// Keep Julia on the team as an admin (`Some(true)`), a member
/// (`Some(false)`), or off it (`None`).
fn set_julia(repo: &FakeRepo, admin: Option<bool>) {
    let mut teams = repo.teams.lock().expect("teams");
    match admin {
        Some(admin) => {
            teams.insert(
                julia().to_string(),
                TeamMembership {
                    team_id: TEAM,
                    admin,
                },
            );
        }
        None => {
            teams.remove(&julia().to_string());
        }
    }
}

/// What a test drives: the records, GitHub, the started workflows, the Run
/// API and the scoring the scoring activity calls.
struct Bench {
    repo: FakeRepo,
    github: FakeGithub,
    workflows: FakeWorkflows,
    judge: Arc<FakeJudge>,
    service: Arc<
        crate::domain::service::FactoryServiceImpl<
            FakeRepo,
            FakeWorkflows,
            super::FakeExecutor,
            FakeRepo,
            FakeRepo,
            FakeGithub,
        >,
    >,
}

impl Bench {
    fn new(repo: FakeRepo, agents: &str, rules: Option<&str>) -> Self {
        let github = FakeGithub::default();
        definition(&github, agents, rules);
        let workflows = FakeWorkflows::default();
        let service = Arc::new(code_service(&repo, &workflows, &github));
        Self {
            repo,
            github,
            workflows,
            judge: Arc::new(FakeJudge::default()),
            service,
        }
    }

    fn improvement(
        &self,
    ) -> SelfImprovementServiceImpl<
        FakeRepo,
        crate::domain::service::FactoryServiceImpl<
            FakeRepo,
            FakeWorkflows,
            super::FakeExecutor,
            FakeRepo,
            FakeRepo,
            FakeGithub,
        >,
        FakeGithub,
    > {
        SelfImprovementServiceImpl::new(
            self.repo.clone(),
            self.service.clone(),
            self.github.clone(),
            APP,
        )
    }

    /// The scoring activity's service: the Scorers, then self-improvement.
    fn scoring(&self) -> impl ScoringService {
        ImprovingScoring::new(
            ScoringServiceImpl::new(
                self.repo.clone(),
                self.judge.clone(),
                FakeUsage::new(Vec::new()),
                self.github.clone(),
                Arc::new(EvaluationDefinition::default()),
            ),
            self.improvement(),
        )
    }

    /// A notes Run of Teo's that ended `success`, judged `score`.
    async fn judged_run(&self, score: f64) -> RunId {
        let run = self
            .service
            .create_run(
                teo(),
                "notes".to_string(),
                "Note it".to_string(),
                None,
                None,
                true,
            )
            .await
            .expect("a notes run");
        self.repo
            .end_run(run.run.id, RunOutcome::Success)
            .await
            .expect("end");
        self.judge.then(score);
        self.scoring()
            .score_run(run.run.id, false)
            .await
            .expect("scored");
        run.run.id
    }

    /// The self-improvement Runs started, in order.
    fn improvement_runs(&self) -> Vec<RunId> {
        let runs = self.repo.runs.lock().expect("runs");
        let mut improving: Vec<_> = runs
            .values()
            .filter(|run| run.flow.as_deref() == Some(IMPROVE_FLOW))
            .map(|run| (run.created_at, run.id))
            .collect();
        improving.sort();
        improving.into_iter().map(|(_, id)| id).collect()
    }

    /// Take the team's latest self-improvement Run as settled `hours` ago.
    fn settled_hours_ago(&self, hours: i64) {
        let mut kept = self.repo.improvement.lock().expect("improvement");
        let latest = kept.improvements.last_mut().expect("an improvement");
        latest.settled_at = Some(Utc::now() - TimeDelta::hours(hours));
    }

    fn latest_state(&self) -> Option<ImprovementState> {
        self.repo
            .improvement
            .lock()
            .expect("improvement")
            .improvements
            .last()
            .map(|latest| latest.state)
    }

    async fn pending(&self) -> u32 {
        self.repo
            .pending_failures(TEAM, DEFINITION_REPO)
            .await
            .expect("pending")
            .count
    }
}

/// Failures below the pass score are kept per team; once they reach the
/// Definition's count, one self-improvement Run starts, as the admin who
/// named the Definition, on its repository, its request listing the
/// failures under "Regressions addressed"; while it is open, more failures
/// wait and no second Run starts.
#[tokio::test]
async fn failures_reaching_the_threshold_start_one_improvement_run() {
    let bench = Bench::new(team_repo(true), AGENTS, None);

    // A judgment at the pass score or above is no failure.
    bench.judged_run(0.9).await;
    let first = bench.judged_run(0.2).await;
    assert_eq!(bench.pending().await, 1);
    assert!(bench.improvement_runs().is_empty());

    let second = bench.judged_run(0.1).await;
    let started = bench.improvement_runs();
    assert_eq!(started.len(), 1, "one self-improvement run");
    let run = bench
        .repo
        .runs
        .lock()
        .expect("runs")
        .get(&started[0])
        .cloned()
        .expect("kept");
    assert_eq!(run.user_id, julia());
    assert_eq!(
        run.repository
            .as_ref()
            .map(|repository| repository.url.as_str()),
        Some(DEFINITION_REPO)
    );
    assert!(!run.created_in_person);
    assert!(run.request.contains("## Regressions addressed"));
    for failed in [first, second] {
        assert!(
            run.request
                .contains(&format!("[Run {failed}]({APP}/app/factory/runs/{failed})")),
            "{failed}"
        );
    }
    assert!(run.request.contains("### Scorer `reads-well` (2"));
    assert_eq!(bench.pending().await, 0);
    let latest = bench
        .repo
        .latest_improvement(TEAM)
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(latest.state, ImprovementState::Open);
    assert_eq!(latest.run_id, Some(started[0]));
    assert_eq!(latest.failures, 2);

    // Its own judgments are no failure of the Definition's.
    bench
        .repo
        .evaluation
        .lock()
        .expect("evaluation")
        .run_scores
        .insert(
            started[0],
            vec![factory_records::domain::evaluation::Judgment {
                scorer: "reads-well".to_string(),
                score: 0.0,
                reason: "Wrong".to_string(),
                model: "judge".to_string(),
            }],
        );
    bench
        .improvement()
        .after_run_end(started[0])
        .await
        .expect("after");
    assert_eq!(bench.pending().await, 0);

    // Two more failures while it runs start nothing.
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    assert_eq!(bench.improvement_runs().len(), 1);
    assert_eq!(bench.pending().await, 2);
}

/// A Run of the improve Flow is Tampered by any change to the Definition's
/// files, so neither the merge rule nor the Orchestrator merges it, even
/// with the team's rules set to merge on green: only a person does.
#[tokio::test]
async fn an_improvement_run_merges_only_by_a_person() {
    let rules = r#"{ "standingRules": { "plan": true, "blockers": true, "merge": true } }"#;
    let bench = Bench::new(team_repo(true), AGENTS, Some(rules));
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    let started = bench.improvement_runs();
    assert_eq!(started.len(), 1);
    let run = bench
        .repo
        .runs
        .lock()
        .expect("runs")
        .get(&started[0])
        .cloned()
        .expect("kept");
    let patterns = run.control_file_patterns.clone().expect("patterns");
    for pattern in crate::domain::definition::DEFINITION_FILE_PATTERNS {
        assert!(patterns.iter().any(|kept| kept == pattern), "{pattern}");
    }
    let rules = run.standing_rules.clone().expect("rules");
    assert!(rules.definition.merge, "the team's rules merge on green");

    // Published through the worker's Steps, its change to a Flow's prompt
    // is Tampered, though its request names the folder: no rule answers
    // its merge Gate, nor its plan's.
    bench.github.state().changed = Some(vec!["flows/notes.json".to_string()]);
    let worker = StepServiceImpl::new(
        bench.repo.clone(),
        BuiltInSteps::new(
            LocalAgent,
            PullRequestSteps::new(
                bench.github.clone(),
                bench.repo.clone(),
                FakeWatches::default(),
            ),
        ),
    );
    let publish = StepRequest {
        key: StepKey {
            run_id: run.id,
            seq: 1,
        },
        entry: EntryId::try_from("publish".to_string()).expect("valid"),
        step: StepName::try_from("publish".to_string()).expect("valid"),
        role: None,
        visit: 1,
        round: None,
        may_ask: false,
        questions_left: 0,
        resumes: None,
    };
    worker.run_step(publish).await.expect("published");
    let kept = bench
        .repo
        .control_file_changes
        .lock()
        .expect("control files")
        .get(&run.id)
        .cloned()
        .expect("control files kept");
    assert_eq!(kept.tampered, vec!["flows/notes.json".to_string()]);
    for kind in [RuleGateKind::Merge, RuleGateKind::Plan] {
        let facts = GateFacts {
            kind,
            fix_rounds: 0,
            fix_limit: 2,
            tampered: kept.is_tampered(),
            held: false,
            repeated_blockers: false,
        };
        let offered = [
            Outcome::try_from("approve".to_string()).expect("valid"),
            Outcome::try_from("revise".to_string()).expect("valid"),
        ];
        assert_eq!(
            rule_answer(Some(rules.definition), &facts, &offered),
            None,
            "{kind:?}"
        );
    }
}

/// A pull request a person merged after its Run ended addresses the
/// Run's failures; one closed unmerged, or a Run cancelled with none, lets
/// them go back to pending but starts no other Run at once: the next waits
/// a day, two after a second such Run in a row, and a merge ends the wait.
#[tokio::test]
async fn the_pull_request_settles_the_failures() {
    let bench = Bench::new(team_repo(true), AGENTS, None);
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    let first = bench.improvement_runs()[0];

    // Ended for a person with its pull request still open: it stays open.
    bench
        .repo
        .end_run(first, RunOutcome::Manual)
        .await
        .expect("end");
    bench.github.state().pull_request = Some(PullRequest {
        number: 7,
        url: "https://github.com/macro/factory-definition/pull/7".to_string(),
        state: PullRequestState::Open,
        head_sha: HEAD.to_string(),
    });
    bench
        .improvement()
        .after_run_end(first)
        .await
        .expect("after");
    assert_eq!(bench.latest_state(), Some(ImprovementState::Open));

    // A person merges it after the Run ended: its failures are addressed.
    if let Some(pull_request) = bench.github.state().pull_request.as_mut() {
        pull_request.state = PullRequestState::Merged;
    }
    bench
        .improvement()
        .after_run_end(first)
        .await
        .expect("after");
    assert_eq!(bench.latest_state(), Some(ImprovementState::Merged));
    let addressed = || {
        bench
            .repo
            .improvement
            .lock()
            .expect("improvement")
            .failures
            .iter()
            .filter(|failure| failure.addressed)
            .count()
    };
    assert_eq!(addressed(), 2);
    assert_eq!(bench.pending().await, 0);

    // After a merge, new failures start the next at once.
    bench.github.state().pull_request = None;
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    assert_eq!(bench.improvement_runs().len(), 2);
    let second = bench.improvement_runs()[1];

    // Cancelled with no pull request: closed, its failures back, and no
    // other starts, though they are due.
    bench
        .repo
        .end_run(second, RunOutcome::Cancelled)
        .await
        .expect("end");
    bench
        .improvement()
        .after_run_end(second)
        .await
        .expect("after");
    assert_eq!(bench.latest_state(), Some(ImprovementState::Closed));
    assert_eq!(bench.pending().await, 2);
    assert_eq!(bench.improvement_runs().len(), 2, "no other at once");
    bench.judged_run(0.2).await;
    assert_eq!(bench.improvement_runs().len(), 2, "none within the day");

    // A day later, the next Run's end starts one.
    bench.settled_hours_ago(25);
    bench.judged_run(0.9).await;
    assert_eq!(bench.improvement_runs().len(), 3);
    let third = bench.improvement_runs()[2];

    // A person closes its pull request: a second unmerged one in a row,
    // so the next waits two days.
    bench
        .repo
        .end_run(third, RunOutcome::Manual)
        .await
        .expect("end");
    bench.github.state().pull_request = Some(PullRequest {
        number: 8,
        url: "https://github.com/macro/factory-definition/pull/8".to_string(),
        state: PullRequestState::Closed,
        head_sha: HEAD.to_string(),
    });
    bench
        .improvement()
        .after_run_end(third)
        .await
        .expect("after");
    assert_eq!(bench.latest_state(), Some(ImprovementState::Closed));
    bench.settled_hours_ago(25);
    bench.judged_run(0.9).await;
    assert_eq!(bench.improvement_runs().len(), 3, "two days after two");
    bench.settled_hours_ago(49);
    bench.judged_run(0.9).await;
    assert_eq!(bench.improvement_runs().len(), 4);

    // Merged by its merge Step: addressed, and the wait is over.
    let fourth = bench.improvement_runs()[3];
    bench
        .repo
        .end_run(fourth, RunOutcome::Success)
        .await
        .expect("end");
    bench
        .improvement()
        .after_run_end(fourth)
        .await
        .expect("after");
    assert_eq!(bench.latest_state(), Some(ImprovementState::Merged));
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    assert_eq!(bench.improvement_runs().len(), 5);
}

/// A Run marked flawed is a failure of its Definition; a failure as old as
/// the Definition says starts a Run with fewer than its count.
#[tokio::test]
async fn a_flawed_run_counts_and_an_old_failure_is_due() {
    let bench = Bench::new(team_repo(true), AGENTS, None);
    let run = bench.judged_run(0.9).await;
    bench
        .service
        .mark_run_flawed(&teo(), run, "The notes miss the point")
        .await
        .expect("marked");
    assert_eq!(bench.pending().await, 1);
    let kept = bench
        .repo
        .improvement
        .lock()
        .expect("improvement")
        .failures
        .clone();
    assert_eq!(kept[0].failure.source, FailureSource::Flawed);

    // One failure, a week old: the next Run's end starts a Run.
    bench.repo.keep_failure_at(
        TEAM,
        DEFINITION_REPO,
        run,
        NewFailure {
            source: FailureSource::Scorer {
                name: "reads-well".to_string(),
                score: 0.3,
            },
            reason: "Old".to_string(),
        },
        Utc::now() - TimeDelta::days(8),
    );
    bench.judged_run(0.9).await;
    assert_eq!(bench.improvement_runs().len(), 1);
}

/// A team that names no Definition keeps no failure and starts nothing; a
/// Definition no one is kept as naming starts nothing either, though its
/// failures are kept.
#[tokio::test]
async fn no_definition_or_no_namer_starts_nothing() {
    let bench = Bench::new(team_repo(false), AGENTS, None);
    bench.judged_run(0.1).await;
    bench.judged_run(0.1).await;
    assert!(bench.improvement_runs().is_empty());
    assert_eq!(bench.pending().await, 2);

    let repo = team_repo(true);
    repo.team_definitions
        .lock()
        .expect("team definitions")
        .remove(&TEAM);
    let bench = Bench::new(repo, AGENTS, None);
    let run = bench
        .service
        .create_run(
            teo(),
            "plan".to_string(),
            "Plan it".to_string(),
            None,
            None,
            true,
        )
        .await
        .expect("a built-in run");
    bench
        .repo
        .end_run(run.run.id, RunOutcome::Success)
        .await
        .expect("end");
    bench
        .repo
        .evaluation
        .lock()
        .expect("evaluation")
        .run_scores
        .insert(
            run.run.id,
            vec![factory_records::domain::evaluation::Judgment {
                scorer: "any".to_string(),
                score: 0.0,
                reason: "Wrong".to_string(),
                model: "judge".to_string(),
            }],
        );
    bench
        .improvement()
        .after_run_end(run.run.id)
        .await
        .expect("after");
    assert!(
        bench
            .repo
            .improvement
            .lock()
            .expect("improvement")
            .failures
            .is_empty()
    );
}

/// A Definition whose Agents do not fill the improve Flow refuses the Run:
/// the refusal is kept with why, its failures go back, and no other Run is
/// tried before a day passed. No one starts a Run of the improve Flow by
/// name.
#[tokio::test]
async fn a_refused_improvement_run_waits_and_people_cannot_start_one() {
    let writers_only = r#"{
        "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
        "roles": { "writer": ["primary"] }
    }"#;
    let bench = Bench::new(team_repo(true), writers_only, None);
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    assert!(bench.improvement_runs().is_empty());
    let latest = bench
        .repo
        .latest_improvement(TEAM)
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(latest.state, ImprovementState::Refused);
    assert!(
        latest
            .refusal
            .as_deref()
            .is_some_and(|why| why.contains("improve flow")),
        "{:?}",
        latest.refusal
    );
    assert_eq!(bench.pending().await, 2);
    bench.judged_run(0.2).await;
    let improvements = bench
        .repo
        .improvement
        .lock()
        .expect("improvement")
        .improvements
        .len();
    assert_eq!(improvements, 1, "no second try within the day");

    let by_name = bench
        .service
        .create_run(
            julia(),
            IMPROVE_FLOW.to_string(),
            "Improve it".to_string(),
            None,
            Some(DEFINITION_REPO.to_string()),
            true,
        )
        .await;
    assert!(
        matches!(by_name, Err(FactoryError::UnknownFlow)),
        "{by_name:?}"
    );
    let listed = bench.service.list_flows(&julia()).await.expect("flows");
    assert!(listed.iter().all(|flow| flow.name != IMPROVE_FLOW));
    assert!(bench.workflows.started.lock().expect("started").len() >= 3);
}

/// Insights show the team's pending failures, by source, what starts a
/// Run, and the latest self-improvement Run; a team with no Definition
/// shows none.
#[tokio::test]
async fn insights_show_pending_failures_and_the_last_improvement_run() {
    use crate::domain::evaluation::insights::{EvaluationService, EvaluationServiceImpl};
    use crate::domain::insights::InsightsQuery;
    use crate::domain::self_improvement::NamerStanding;

    let bench = Bench::new(team_repo(true), AGENTS, None);
    let today = Utc::now().date_naive();
    let query = || InsightsQuery::new(today, today, "UTC", None).expect("valid query");
    let insights = EvaluationServiceImpl::new(
        bench.repo.clone(),
        bench.service.clone(),
        Arc::new(EvaluationDefinition::default()),
        None,
    );

    bench.judged_run(0.2).await;
    let shown = insights
        .evaluation(&teo(), query())
        .await
        .expect("insights")
        .self_improvement
        .expect("the team names a definition");
    assert_eq!(shown.repo_url, DEFINITION_REPO);
    assert_eq!(shown.pending, 1);
    assert_eq!(shown.failures_threshold, 2);
    assert_eq!(shown.oldest_days, 7);
    assert!(shown.enabled);
    assert_eq!(shown.namer, NamerStanding::Admin);
    assert_eq!(shown.pending_by_source[0].source, "reads-well");
    assert_eq!(shown.last, None);

    bench.judged_run(0.2).await;
    let shown = insights
        .evaluation(&teo(), query())
        .await
        .expect("insights")
        .self_improvement
        .expect("the team names a definition");
    assert_eq!(shown.pending, 0);
    let last = shown.last.expect("the last run");
    assert_eq!(last.state, ImprovementState::Open);
    assert_eq!(last.failures, 2);
    assert_eq!(last.run_id, bench.improvement_runs().first().copied());

    // The namer demoted, then gone from the team, then none kept.
    let namer = || async {
        insights
            .evaluation(&teo(), query())
            .await
            .expect("insights")
            .self_improvement
            .expect("the team names a definition")
            .namer
    };
    set_julia(&bench.repo, Some(false));
    assert_eq!(namer().await, NamerStanding::NotAdmin);
    set_julia(&bench.repo, None);
    assert_eq!(namer().await, NamerStanding::LeftTeam);
    bench
        .repo
        .definition_namers
        .lock()
        .expect("namers")
        .remove(&TEAM);
    assert_eq!(namer().await, NamerStanding::None);

    bench
        .repo
        .team_definitions
        .lock()
        .expect("team definitions")
        .remove(&TEAM);
    let none = insights
        .evaluation(&teo(), query())
        .await
        .expect("insights");
    assert_eq!(none.self_improvement, None);
}

/// Whatever an improve Run changes, even only a file outside the
/// Definition's (no Control File, so not Tampered), its plan and merge
/// Gates are a person's: no Standing Rule answers them, the stand-in may
/// not, and the Orchestrator may not approve its plan or its merge, though
/// the team's rules turn all of them on.
#[tokio::test]
async fn an_improvement_run_s_plan_and_merge_wait_for_a_person_whatever_it_changes() {
    use crate::domain::gate::GateRuling;
    use crate::domain::models::Answerer;
    use crate::domain::stand_in::{StandInFacts, may_stand_in};

    let rules = r#"{ "standingRules": {
        "plan": true, "blockers": true, "merge": true,
        "standIn": { "plan": true, "blockers": true },
        "orchestrator": { "plan": true, "blockers": true }
    } }"#;
    let bench = Bench::new(team_repo(true), AGENTS, Some(rules));
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    let started = bench.improvement_runs();
    assert_eq!(started.len(), 1);
    let run_id = started[0];

    // It changes only the README: nothing Tampered.
    bench.github.state().changed = Some(vec!["README.md".to_string()]);
    let worker = StepServiceImpl::new(
        bench.repo.clone(),
        BuiltInSteps::new(
            LocalAgent,
            PullRequestSteps::new(
                bench.github.clone(),
                bench.repo.clone(),
                FakeWatches::default(),
            ),
        ),
    );
    let publish = StepRequest {
        key: StepKey { run_id, seq: 1 },
        entry: EntryId::try_from("publish".to_string()).expect("valid"),
        step: StepName::try_from("publish".to_string()).expect("valid"),
        role: None,
        visit: 1,
        round: None,
        may_ask: false,
        questions_left: 0,
        resumes: None,
    };
    worker.run_step(publish).await.expect("published");
    let kept = bench
        .repo
        .get_run(run_id)
        .await
        .expect("read")
        .expect("kept");
    assert!(
        !kept
            .control_files
            .as_ref()
            .is_some_and(|changes| changes.is_tampered()),
        "a README change is no Control File's"
    );
    let rules = kept.standing_rules.expect("rules").definition;
    assert!(rules.plan && rules.merge && rules.stand_in.plan && rules.orchestrator.plan);

    let flow = bench
        .service
        .run_flow(run_id)
        .await
        .expect("read")
        .expect("its flow");
    let offered = [
        Outcome::try_from("approve".to_string()).expect("valid"),
        Outcome::try_from("revise".to_string()).expect("valid"),
    ];
    for (seq, entry, kind) in [
        (2, "plan", RuleGateKind::Plan),
        (3, "merge-gate", RuleGateKind::Merge),
    ] {
        let key = StepKey { run_id, seq };
        let entry_id = EntryId::try_from(entry.to_string()).expect("valid");
        bench
            .repo
            .start_step(key, &entry_id, 1)
            .await
            .expect("start");
        bench
            .repo
            .open_gate(key, "Approve?", &offered)
            .await
            .expect("open");

        // No Standing Rule answers it.
        let ruled = worker
            .rule_gate(GateRuling {
                key,
                kind,
                fix_rounds: 0,
                fix_limit: 0,
                answers: offered.to_vec(),
            })
            .await
            .expect("rule");
        assert_eq!(ruled, None, "{entry}");

        // The stand-in is never asked: its Flow keeps the Gate for a person.
        let gate = flow
            .entries
            .iter()
            .find(|asking| asking.id == entry_id)
            .and_then(|asking| asking.gate.as_ref())
            .expect("the entry's gate");
        assert_eq!(gate.kind, Some(kind));
        let facts = StandInFacts {
            kind,
            person_only: gate.person_only,
            tampered: false,
            held: false,
            fix_rounds: 0,
            fix_limit: 0,
            repeated_blockers: false,
        };
        assert!(!may_stand_in(Some(rules), &facts, &offered), "{entry}");

        // Nor does the Orchestrator approve it.
        let by_agent = bench
            .service
            .answer_gate(
                julia(),
                run_id,
                seq,
                offered[0].clone(),
                None,
                None,
                Answerer::Agent {
                    session_id: Uuid::now_v7(),
                },
            )
            .await;
        assert!(
            matches!(by_agent, Err(FactoryError::ReservedForAPerson(_))),
            "{entry}: {by_agent:?}"
        );
    }

    // The merge waits for a person, who may give it.
    let merge_gate = bench
        .repo
        .list_steps(run_id)
        .await
        .expect("steps")
        .into_iter()
        .find(|step| step.seq == 3)
        .and_then(|step| step.gate)
        .expect("the merge gate");
    assert_eq!(merge_gate.answer, None);
}

/// The namer starts a self-improvement Run only while an admin of the
/// team: demoted, or gone from it, starts none, and the failures stay
/// pending until an admin names the Definition again.
#[tokio::test]
async fn only_an_admin_namer_starts_an_improvement_run() {
    for admin in [Some(false), None] {
        let bench = Bench::new(team_repo(true), AGENTS, None);
        set_julia(&bench.repo, admin);
        bench.judged_run(0.2).await;
        bench.judged_run(0.2).await;
        bench.judged_run(0.2).await;
        assert!(bench.improvement_runs().is_empty(), "{admin:?}");
        assert_eq!(bench.latest_state(), None, "{admin:?}");
        assert_eq!(bench.pending().await, 3, "{admin:?}");

        set_julia(&bench.repo, Some(true));
        bench.judged_run(0.9).await;
        assert_eq!(bench.improvement_runs().len(), 1, "{admin:?}");
    }
}

/// Whether and when a Run starts is the team's Definition's as it is now,
/// as Insights shows it: Runs that kept a copy saying otherwise change
/// nothing. While it is turned off, failures are still kept.
#[tokio::test]
async fn the_definition_as_it_is_now_decides() {
    let bench = Bench::new(team_repo(true), AGENTS, None);
    // The Runs start under the Definition at two failures.
    let first = bench.judged_run(0.2).await;
    assert!(
        bench
            .repo
            .definition_evaluations
            .lock()
            .expect("kept")
            .contains_key(&first),
        "the run keeps the definition's scorers"
    );

    // The Definition then turns self-improvement off.
    let scorers = r#"{
        "scorers": [{
            "name": "reads-well", "instructions": "Do the notes read well?",
            "flows": ["notes"], "samplePercent": 100
        }],
        "selfImprovement": { "enabled": false, "failures": 2, "oldestDays": 7 }
    }"#;
    bench.github.state().files.insert(
        (HEAD.to_string(), "scorers.json".to_string()),
        scorers.to_string(),
    );
    bench
        .improvement()
        .after_run_end(first)
        .await
        .expect("after");
    bench.repo.keep_failure_at(
        TEAM,
        DEFINITION_REPO,
        first,
        NewFailure {
            source: FailureSource::Flawed,
            reason: "Wrong".to_string(),
        },
        Utc::now(),
    );
    bench
        .improvement()
        .after_run_end(first)
        .await
        .expect("after");
    assert!(bench.improvement_runs().is_empty(), "turned off now");
    bench.judged_run(0.2).await;
    assert!(bench.improvement_runs().is_empty());
    assert_eq!(bench.pending().await, 3, "failures are still kept");

    // Turned on again with a higher count: still not due at three.
    let higher = scorers.replace(r#""enabled": false, "failures": 2"#, r#""failures": 4"#);
    bench
        .github
        .state()
        .files
        .insert((HEAD.to_string(), "scorers.json".to_string()), higher);
    bench.judged_run(0.9).await;
    assert!(bench.improvement_runs().is_empty(), "below the count now");
    bench.judged_run(0.2).await;
    assert_eq!(bench.improvement_runs().len(), 1);
}

/// A self-improvement Run kept starting too long is refused, and the next
/// waits a day from then, in the same Run's end.
#[tokio::test]
async fn a_stale_starting_improvement_pauses_the_next() {
    let bench = Bench::new(team_repo(true), AGENTS, None);
    bench.judged_run(0.2).await;
    bench.judged_run(0.2).await;
    {
        let mut kept = bench.repo.improvement.lock().expect("improvement");
        let latest = kept.improvements.last_mut().expect("an improvement");
        latest.state = ImprovementState::Starting;
        latest.run_id = None;
        latest.created_at = Utc::now() - TimeDelta::hours(1);
    }
    bench.judged_run(0.9).await;
    assert_eq!(bench.latest_state(), Some(ImprovementState::Refused));
    assert_eq!(bench.pending().await, 2);
    assert_eq!(
        bench
            .repo
            .improvement
            .lock()
            .expect("improvement")
            .improvements
            .len(),
        1,
        "the refusal pauses the next"
    );
}
