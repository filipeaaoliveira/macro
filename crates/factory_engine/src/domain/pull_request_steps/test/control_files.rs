//! The Control Files a Run's commits change: checked before publishing and
//! before merging, kept on the Run, and approved by a person before a
//! Tampered Run merges.

use chrono::Utc;
use factory_records::domain::models::{
    ControlFileChanges, EntryId, Gate, GateAnswer, GateKind, NewRun, Outcome, RunId, Step, StepEnd,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;

use super::{JULIA, input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::models::PullRequestState;

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

fn tampered(files: &[&str]) -> Option<ControlFileChanges> {
    Some(ControlFileChanges {
        tampered: paths(files),
        named: Vec::new(),
        unlisted: false,
    })
}

/// GitHub with the Run's pull request open and passing, its branch
/// changing `changed`.
fn github_changing(changed: Option<&[&str]>) -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    github.state().changed = changed.map(paths);
    github
}

/// The merge Gate's Step at `seq`, answered `approve`, approving
/// `approved`.
fn merge_gate(seq: u16, approved: Option<&[&str]>) -> Step {
    let approve = Outcome::try_from("approve".to_string()).expect("valid");
    Step {
        seq,
        entry: EntryId::try_from("merge-gate".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "Every check passed.".to_string(),
            ended_at: Utc::now(),
        }),
        gate: Some(Gate {
            asked_back: None,
            stand_in: None,
            stand_in_declined: None,
            kind: GateKind::Flow,
            question: "Merge?".to_string(),
            answers: vec![approve.clone()],
            opened_at: Utc::now(),
            answer: Some(GateAnswer {
                reply_confidence: None,
                by_stand_in: false,
                agent_session_id: None,
                answer: approve,
                note: None,
                user_id: None,
                rule: None,
                approved_control_files: approved.map(paths),
                answered_at: Utc::now(),
            }),
        }),
        checks: Vec::new(),
        terminal: Default::default(),
    }
}

fn kept(repo: &FakeRepo, run_id: RunId) -> Option<ControlFileChanges> {
    repo.control_file_changes
        .lock()
        .expect("control files")
        .get(&run_id)
        .cloned()
}

/// Merge the Run `run_id`'s head, agreed to by the answers of `earlier`.
async fn merge_after(
    github: &FakeGithub,
    repo: &FakeRepo,
    run_id: RunId,
    earlier: &[Step],
) -> String {
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    let mut merging = input(run_id);
    merging.earlier = earlier;
    let report = steps(github, repo).merge(merging).await.expect("merge");
    outcome(&report).to_string()
}

/// Publishing a head that changes Control Files the request does not name
/// keeps them on the Run and names them; the published head is what is
/// compared with the base.
#[tokio::test]
async fn publishing_a_tampered_branch_names_its_control_files() {
    let github = github_changing(Some(&[
        "src/slug.ts",
        ".github/workflows/ci.yml",
        "src/slug.test.ts",
    ]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let report = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");

    assert_eq!(outcome(&report), "published");
    assert!(report.output.contains("- `.github/workflows/ci.yml`"));
    assert!(report.output.contains("- `src/slug.test.ts`"));
    assert_eq!(
        kept(&repo, run_id),
        tampered(&[".github/workflows/ci.yml", "src/slug.test.ts"])
    );
    assert_eq!(github.state().compared, [FakeGithub::HEAD]);
}

/// A later read that finds no Control File, as once a fix undid the change,
/// clears what was kept.
#[tokio::test]
async fn a_branch_that_no_longer_changes_control_files_is_cleared() {
    let github = github_changing(Some(&["src/slug.ts"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_control_file_changes(run_id, tampered(&["package.json"]).as_ref())
        .await
        .expect("keep");

    let report = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");

    assert_eq!(outcome(&report), "published");
    assert!(!report.output.contains("Control Files"));
    assert_eq!(kept(&repo, run_id), None);
}

/// Before a person is asked to merge, the head's Control Files are read
/// again and named with the question, those the request names marked so.
#[tokio::test]
async fn ready_to_merge_names_the_control_files_of_the_head() {
    let github = github_changing(Some(&["src/slug.ts", "biome.jsonc", "Makefile"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    let mut asking = input(run_id);
    asking.request = "Collapse repeated dashes\n\nAdd a target to Makefile.";

    let report = steps(&github, &repo)
        .ready_to_merge(asking)
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "done");
    assert!(report.output.contains("- `biome.jsonc`"));
    assert!(
        report
            .output
            .contains("named by the request:\n\n- `Makefile`")
    );
    assert_eq!(
        kept(&repo, run_id),
        Some(ControlFileChanges {
            tampered: paths(&["biome.jsonc"]),
            named: paths(&["Makefile"]),
            unlisted: false,
        })
    );
    assert_eq!(github.state().compared, [FakeGithub::HEAD]);
}

/// A pull request merged outside the Run keeps the Control Files read
/// while it was open: once merged, GitHub's comparison lists no change, so
/// `publish`, `ready_to_merge` and `merge` name the kept files rather than
/// read them again.
#[tokio::test]
async fn a_pull_request_merged_outside_the_run_keeps_its_control_files() {
    for step in ["publish", "ready_to_merge", "merge"] {
        let github = github_changing(Some(&["package.json"]));
        let repo = FakeRepo::default();
        let run_id = RunId::generate();
        repo.insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Collapse repeated dashes in slugs",
            user_id: &JULIA,
            task_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert");
        repo.set_merge_head(run_id, FakeGithub::HEAD)
            .await
            .expect("keep");
        steps(&github, &repo)
            .publish(input(run_id))
            .await
            .expect("publish");
        if let Some(pull_request) = github.state().pull_request.as_mut() {
            pull_request.state = PullRequestState::Merged;
        }

        let steps = steps(&github, &repo);
        let report = match step {
            "publish" => steps.publish(input(run_id)).await,
            "ready_to_merge" => steps.ready_to_merge(input(run_id)).await,
            _ => steps.merge(input(run_id)).await,
        }
        .expect("step");

        assert!(
            report.output.contains("- `package.json`"),
            "{step} names it"
        );
        assert_eq!(
            kept(&repo, run_id),
            tampered(&["package.json"]),
            "{step} keeps it"
        );
    }
}

/// A Tampered commit merges only once the answer that agreed to it
/// approved every Control File it changes unasked. Otherwise it is
/// `changed`, which takes the Run back through `ci` to the merge Gate, and
/// nothing merges.
#[tokio::test]
async fn a_tampered_commit_merges_only_once_a_person_approved_its_files() {
    let changed: &[&str] = &["src/slug.ts", "package.json", "src/slug.test.ts"];
    let cases: [(Option<&[&str]>, &str); 3] = [
        (None, "changed"),
        (Some(&["package.json"]), "changed"),
        (Some(&["src/slug.test.ts", "package.json"]), "merged"),
    ];

    for (approved, expected) in cases {
        let github = github_changing(Some(changed));
        let repo = FakeRepo::default();

        let merged = merge_after(
            &github,
            &repo,
            RunId::generate(),
            &[merge_gate(4, approved)],
        )
        .await;

        assert_eq!(merged, expected, "approving {approved:?}");
        let merges = github.state().merges.clone();
        if expected == "merged" {
            assert_eq!(merges, [FakeGithub::HEAD]);
        } else {
            assert!(merges.is_empty(), "approving {approved:?} merges nothing");
        }
    }
}

/// Only the latest answer counts: one that approved nothing, after an
/// earlier one that approved the files, merges nothing.
#[tokio::test]
async fn a_later_answer_approving_nothing_overrides_an_earlier_approval() {
    let github = github_changing(Some(&["package.json"]));
    let repo = FakeRepo::default();
    let earlier = [merge_gate(4, Some(&["package.json"])), merge_gate(7, None)];

    let merged = merge_after(&github, &repo, RunId::generate(), &earlier).await;

    assert_eq!(merged, "changed");
    assert!(github.state().merges.is_empty());
}

/// A commit that changes only Control Files its request names needs no
/// approval to merge.
#[tokio::test]
async fn control_files_the_request_names_need_no_approval() {
    let github = github_changing(Some(&["Makefile"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    let earlier = [merge_gate(4, None)];
    let mut merging = input(run_id);
    merging.request = "Add a target to Makefile";
    merging.earlier = &earlier;

    let report = steps(&github, &repo).merge(merging).await.expect("merge");

    assert_eq!(outcome(&report), "merged");
}

/// A branch whose changes GitHub could not list is Tampered, unlisted: it
/// merges only on an approval given while it was.
#[tokio::test]
async fn an_unlisted_branch_merges_only_on_an_approval_of_it() {
    for (approved, expected) in [(None, "changed"), (Some(&[][..]), "merged")] {
        let github = github_changing(None);
        let repo = FakeRepo::default();
        let run_id = RunId::generate();

        let merged = merge_after(&github, &repo, run_id, &[merge_gate(4, approved)]).await;

        assert_eq!(merged, expected);
        assert_eq!(
            kept(&repo, run_id),
            Some(ControlFileChanges {
                tampered: Vec::new(),
                named: Vec::new(),
                unlisted: true,
            })
        );
    }
}

/// The patterns the Run keeps from its creation replace the default: a
/// file only the default names is not a Control File, and an empty list asks
/// GitHub nothing. A Run that keeps none is checked against the default.
#[tokio::test]
async fn the_patterns_the_run_keeps_replace_the_default() {
    let github = github_changing(Some(&["package.json", "ci/build.yml"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let own = paths(&["ci/**"]);
    let none: Vec<String> = Vec::new();

    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    assert_eq!(kept(&repo, run_id), tampered(&["package.json"]));

    let mut with_own = input(run_id);
    with_own.control_file_patterns = Some(&own);
    steps(&github, &repo)
        .publish(with_own)
        .await
        .expect("publish");
    assert_eq!(kept(&repo, run_id), tampered(&["ci/build.yml"]));

    let mut with_none = input(run_id);
    with_none.control_file_patterns = Some(&none);
    let compared = github.state().compared.len();
    steps(&github, &repo)
        .publish(with_none)
        .await
        .expect("publish");
    assert_eq!(kept(&repo, run_id), None);
    assert_eq!(github.state().compared.len(), compared);
}

/// On its team's Definition repository, a change to the Definition's own
/// files is Tampered even when the request, made in person, names it: a
/// person approves every change to the rules the factory merges under.
/// The request still names the other Control Files it asks for.
#[tokio::test]
async fn a_definition_s_own_files_are_tampered_on_its_repository() {
    let github = github_changing(Some(&["rules.json", "Makefile", "src/slug.ts"]));
    let repo = FakeRepo::default();
    let patterns = paths(&["rules.json", "Makefile"]);
    let on_definition = RunId::generate();
    let elsewhere = RunId::generate();
    let asking = |run_id, definition_repo| {
        let mut asking = input(run_id);
        asking.request = "Update rules.json and the Makefile.";
        asking.control_file_patterns = Some(&patterns);
        asking.definition_repo = definition_repo;
        asking
    };

    steps(&github, &repo)
        .publish(asking(on_definition, Some("https://github.com/Macro/App")))
        .await
        .expect("publish");
    steps(&github, &repo)
        .publish(asking(
            elsewhere,
            Some("https://github.com/macro/factory-definition"),
        ))
        .await
        .expect("publish");

    assert_eq!(
        kept(&repo, on_definition),
        Some(ControlFileChanges {
            tampered: paths(&["rules.json"]),
            named: paths(&["Makefile"]),
            unlisted: false,
        })
    );
    assert_eq!(
        kept(&repo, elsewhere),
        Some(ControlFileChanges {
            tampered: Vec::new(),
            named: paths(&["rules.json", "Makefile"]),
            unlisted: false,
        })
    );
}
