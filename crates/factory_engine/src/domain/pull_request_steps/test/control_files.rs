//! The Control Files a Run's commits change: checked before publishing and
//! before merging, kept on the Run, and approved by a person before a
//! Tampered Run merges.

use chrono::Utc;
use factory_records::domain::models::{
    ControlFileChanges, EntryId, Gate, GateAnswer, Outcome, RunId, Step, StepEnd, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

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
            question: "Merge?".to_string(),
            answers: vec![approve.clone()],
            opened_at: Utc::now(),
            answer: Some(GateAnswer {
                answer: approve,
                note: None,
                user_id: None,
                approved_control_files: approved.map(paths),
                answered_at: Utc::now(),
            }),
        }),
        checks: Vec::new(),
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

/// A pull request merged outside the Run still has its Control Files kept,
/// whether `ready_to_merge` or `merge` finds it merged.
#[tokio::test]
async fn a_pull_request_merged_outside_the_run_keeps_its_control_files() {
    for merging in [false, true] {
        let github = github_changing(Some(&["package.json"]));
        if let Some(pull_request) = github.state().pull_request.as_mut() {
            pull_request.state = PullRequestState::Merged;
        }
        let repo = FakeRepo::default();
        let run_id = RunId::generate();
        repo.set_merge_head(run_id, FakeGithub::HEAD)
            .await
            .expect("keep");

        let report = if merging {
            steps(&github, &repo).merge(input(run_id)).await
        } else {
            steps(&github, &repo).ready_to_merge(input(run_id)).await
        }
        .expect("step");

        assert_eq!(outcome(&report), "merged");
        assert!(report.output.contains("- `package.json`"));
        assert_eq!(kept(&repo, run_id), tampered(&["package.json"]));
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

/// The patterns of the owner's team replace the default: a file only the
/// default names is not a Control File, and an empty list asks GitHub
/// nothing. An owner on no team, or whose team set none, has the default.
#[tokio::test]
async fn the_patterns_of_the_owner_s_team_replace_the_default() {
    let github = github_changing(Some(&["package.json", "ci/build.yml"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let team_id = Uuid::now_v7();

    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    assert_eq!(kept(&repo, run_id), tampered(&["package.json"]));

    repo.teams.lock().expect("teams").insert(
        JULIA.to_string(),
        TeamMembership {
            team_id,
            admin: false,
        },
    );
    repo.set_team_control_files(team_id, Some(&paths(&["ci/**"])))
        .await
        .expect("set");
    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    assert_eq!(kept(&repo, run_id), tampered(&["ci/build.yml"]));

    repo.set_team_control_files(team_id, Some(&[]))
        .await
        .expect("set");
    let compared = github.state().compared.len();
    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    assert_eq!(kept(&repo, run_id), None);
    assert_eq!(github.state().compared.len(), compared);
}
