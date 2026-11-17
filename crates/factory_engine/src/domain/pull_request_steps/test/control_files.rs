//! The Control Files a Run's commits change: checked before publishing and
//! before merging, kept on the Run, and approved by a person before a
//! Tampered Run merges.

use chrono::Utc;
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, Outcome, RunId, Step, StepEnd, Tampering,
};
use factory_records::domain::ports::RunRepository;

use super::{JULIA, input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

/// GitHub with the Run's pull request open and passing, its branch
/// changing `changed`.
fn github_changing(changed: Option<&[&str]>) -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    github.state().changed = changed.map(paths);
    github
}

/// The merge Gate's Step, answered `approve`, approving `approved`.
fn merge_gate(approved: Option<&[&str]>) -> Step {
    let approve = Outcome::try_from("approve".to_string()).expect("valid");
    Step {
        seq: 4,
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

fn tampering(repo: &FakeRepo, run_id: RunId) -> Option<Tampering> {
    repo.tampering
        .lock()
        .expect("tampering")
        .get(&run_id)
        .cloned()
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
        tampering(&repo, run_id),
        Some(Tampering {
            files: paths(&[".github/workflows/ci.yml", "src/slug.test.ts"]),
            unlisted: false,
        })
    );
    assert_eq!(github.state().compared, [FakeGithub::HEAD]);
}

/// A later read that finds no Control File the request does not name, as
/// once a fix undid the change, clears what was kept.
#[tokio::test]
async fn a_branch_that_no_longer_tampers_is_cleared() {
    let github = github_changing(Some(&["src/slug.ts"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let before = Tampering {
        files: paths(&["package.json"]),
        unlisted: false,
    };
    repo.set_tampering(run_id, Some(&before))
        .await
        .expect("keep");

    let report = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");

    assert_eq!(outcome(&report), "published");
    assert!(!report.output.contains("Control Files"));
    assert_eq!(tampering(&repo, run_id), None);
}

/// Before a person is asked to merge, the head's Control Files are read
/// again and named with the question.
#[tokio::test]
async fn ready_to_merge_names_the_control_files_of_the_head() {
    let github = github_changing(Some(&["src/slug.ts", "biome.jsonc"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");

    let report = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "done");
    assert!(report.output.contains("- `biome.jsonc`"));
    assert_eq!(
        tampering(&repo, run_id),
        Some(Tampering {
            files: paths(&["biome.jsonc"]),
            unlisted: false,
        })
    );
    assert_eq!(github.state().compared, [FakeGithub::HEAD]);
}

/// A Tampered commit merges only once the answer that agreed to it
/// approved every Control File it changes: an answer that approved none,
/// or only some, merges nothing.
#[tokio::test]
async fn a_tampered_commit_merges_only_once_a_person_approved_its_files() {
    let changed: &[&str] = &["src/slug.ts", "package.json", "src/slug.test.ts"];
    let cases: [(Option<&[&str]>, &str); 3] = [
        (None, "refused"),
        (Some(&["package.json"]), "refused"),
        (Some(&["src/slug.test.ts", "package.json"]), "merged"),
    ];

    for (approved, expected) in cases {
        let github = github_changing(Some(changed));
        let repo = FakeRepo::default();
        let run_id = RunId::generate();
        repo.set_merge_head(run_id, FakeGithub::HEAD)
            .await
            .expect("keep");
        let earlier = [merge_gate(approved)];
        let mut merging = input(run_id);
        merging.earlier = &earlier;

        let report = steps(&github, &repo).merge(merging).await.expect("merge");

        assert_eq!(outcome(&report), expected, "approving {approved:?}");
        let merged = github.state().merges.clone();
        if expected == "merged" {
            assert_eq!(merged, [FakeGithub::HEAD]);
        } else {
            assert!(merged.is_empty(), "approving {approved:?} merges nothing");
            assert!(report.output.contains("- `package.json`"));
        }
    }
}

/// A branch whose changes GitHub could not list is Tampered, unlisted: it
/// merges only on an approval given while it was.
#[tokio::test]
async fn an_unlisted_branch_merges_only_on_an_approval_of_it() {
    for (approved, expected) in [(None, "refused"), (Some(&[][..]), "merged")] {
        let github = github_changing(None);
        let repo = FakeRepo::default();
        let run_id = RunId::generate();
        repo.set_merge_head(run_id, FakeGithub::HEAD)
            .await
            .expect("keep");
        let earlier = [merge_gate(approved)];
        let mut merging = input(run_id);
        merging.earlier = &earlier;

        let report = steps(&github, &repo).merge(merging).await.expect("merge");

        assert_eq!(outcome(&report), expected);
        assert_eq!(
            tampering(&repo, run_id),
            Some(Tampering {
                files: Vec::new(),
                unlisted: true,
            })
        );
    }
}

/// The owner's own patterns replace the default: a file only the default
/// names is not a Control File, and an empty list asks GitHub nothing.
#[tokio::test]
async fn the_owner_s_own_patterns_replace_the_default() {
    let github = github_changing(Some(&["package.json", "ci/build.yml"]));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_control_files(&JULIA, Some(&paths(&["ci/**"])))
        .await
        .expect("set");

    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    assert_eq!(
        tampering(&repo, run_id),
        Some(Tampering {
            files: paths(&["ci/build.yml"]),
            unlisted: false,
        })
    );

    repo.set_control_files(&JULIA, Some(&[]))
        .await
        .expect("set");
    let compared = github.state().compared.len();
    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    assert_eq!(tampering(&repo, run_id), None);
    assert_eq!(github.state().compared.len(), compared);
}
