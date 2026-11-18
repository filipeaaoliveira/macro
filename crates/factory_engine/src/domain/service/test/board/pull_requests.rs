//! The Runs that opened some pull requests: only those the person sees on
//! their team's board, the newest of each, and the merge Gate one waits on.

use factory_records::domain::board::BoardState;
use factory_records::domain::models::{
    EntryId, NewRun, Outcome, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;

use super::super::code_service;
use super::{FakeWorkflows, ana, hide, julia, keep, on_team, service, task, teo};
use crate::domain::board::{PULL_REQUESTS_MAX, PullRequestRuns};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

const COUPON: &str = "https://github.com/acme/shop/pull/7";
const BANNER: &str = "https://github.com/acme/shop/pull/8";
const CART: &str = "https://github.com/acme/shop/pull/9";
const LOGIN: &str = "https://github.com/acme/shop/pull/10";
const RECEIPT: &str = "https://github.com/acme/shop/pull/11";

async fn opened(repo: &FakeRepo, id: RunId, url: &str) -> RunId {
    repo.set_pull_request_url(id, url).await.expect("keep");
    id
}

fn shown(found: &PullRequestRuns) -> Vec<(String, RunId)> {
    found
        .runs
        .iter()
        .map(|found| (found.url.clone(), found.listed.detail.run.id))
        .collect()
}

/// Each pull request shows the newest Run the person sees on their team's
/// board, under the page they named, in their order. A team member's Run
/// with no task or on a task hidden from them, and a Run off their team,
/// read as no Run at all: the same as a pull request no Run opened.
#[tokio::test]
async fn a_pull_request_shows_only_a_run_the_person_sees() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let shared_task = task(&repo);
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);

    let own = opened(&repo, keep(&repo, &julia(), None).await, COUPON).await;
    let shared = opened(&repo, keep(&repo, &teo(), Some(shared_task)).await, BANNER).await;
    // A newer Run of the same pull request on a task hidden from her
    // leaves the one she sees.
    let _hidden_newer = opened(&repo, keep(&repo, &teo(), Some(hidden_task)).await, BANNER).await;
    let _hidden = opened(&repo, keep(&repo, &teo(), Some(hidden_task)).await, CART).await;
    let _no_task = opened(&repo, keep(&repo, &teo(), None).await, LOGIN).await;
    let _off_team = opened(&repo, keep(&repo, &ana(), Some(shared_task)).await, RECEIPT).await;

    let found = service
        .pull_request_runs(
            &julia(),
            vec![
                "https://github.com/Acme/Shop/pull/8".to_string(),
                CART.to_string(),
                LOGIN.to_string(),
                RECEIPT.to_string(),
                "https://github.com/acme/shop/pull/12".to_string(),
                COUPON.to_string(),
                COUPON.to_string(),
            ],
        )
        .await
        .expect("found");
    assert_eq!(
        shown(&found),
        [
            ("https://github.com/Acme/Shop/pull/8".to_string(), shared),
            (COUPON.to_string(), own),
        ]
    );
    assert!(
        found
            .runs
            .iter()
            .all(|found| found.listed.state == BoardState::Running)
    );

    // Teo sees his own Runs, the newest first.
    let teos = service
        .pull_request_runs(&teo(), vec![BANNER.to_string(), COUPON.to_string()])
        .await
        .expect("found");
    assert_eq!(teos.runs.len(), 1);
    assert_ne!(teos.runs[0].listed.detail.run.id, shared);
}

/// A Run waiting at its merge Gate names the Step it waits on; a Run at
/// another Gate, or one that ended, names none.
#[tokio::test]
async fn a_run_at_its_merge_gate_names_it() {
    let repo = FakeRepo::default();
    let service = code_service(&repo, &FakeWorkflows::default(), &FakeGithub::default());
    let at = |entry: &str| {
        let repo = repo.clone();
        let entry = EntryId::try_from(entry.to_string()).expect("valid");
        async move {
            let RunInsert::Inserted(run) = repo
                .insert_run(NewRun {
                    id: RunId::generate(),
                    flow: "code",
                    request: "Add the coupon field",
                    user_id: &julia(),
                    task_id: None,
                    project_id: None,
                    repository: None,
                    created_in_person: true,
                    control_file_patterns: &[],
                    control_files_team_id: None,
                    standing_rules: RunRules::DEFAULT,
                    automation: None,
                })
                .await
                .expect("insert")
            else {
                panic!("a run with no task is kept");
            };
            let key = StepKey {
                run_id: run.id,
                seq: 1,
            };
            let done = Outcome::try_from("done".to_string()).expect("valid");
            let answers = [
                Outcome::try_from("approve".to_string()).expect("valid"),
                Outcome::try_from("revise".to_string()).expect("valid"),
            ];
            repo.start_step(key, &entry, 1).await.expect("start");
            repo.end_step(key, &done, "Ready").await.expect("end");
            repo.open_gate(key, "Merge?", &answers).await.expect("open");
            run.id
        }
    };
    let merging = opened(&repo, at("merge-gate").await, COUPON).await;
    let planning = opened(&repo, at("plan").await, BANNER).await;
    let ended = opened(&repo, at("merge-gate").await, CART).await;
    repo.end_run(ended, RunOutcome::Manual).await.expect("end");

    let found = service
        .pull_request_runs(
            &julia(),
            vec![COUPON.to_string(), BANNER.to_string(), CART.to_string()],
        )
        .await
        .expect("found");
    let gates: Vec<(RunId, Option<u16>)> = found
        .runs
        .iter()
        .map(|found| (found.listed.detail.run.id, found.merge_gate_seq))
        .collect();
    assert_eq!(gates, [(merging, Some(1)), (planning, None), (ended, None)]);
}

/// A lookup names at most so many pull requests, and none reads nothing.
#[tokio::test]
async fn a_lookup_names_at_most_so_many_pull_requests() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());

    let urls = (0..=PULL_REQUESTS_MAX)
        .map(|number| format!("https://github.com/acme/shop/pull/{number}"))
        .collect();
    let refused = service.pull_request_runs(&julia(), urls).await;
    assert!(matches!(
        refused,
        Err(FactoryError::TooManyPullRequests {
            max: PULL_REQUESTS_MAX
        })
    ));

    let none = service
        .pull_request_runs(&julia(), Vec::new())
        .await
        .expect("found");
    assert!(none.runs.is_empty());
}
