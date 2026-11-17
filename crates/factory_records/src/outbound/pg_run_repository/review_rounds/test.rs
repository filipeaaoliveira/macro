use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::models::{EntryId, Outcome, RunId, RunInsert, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::review_rounds::{
    Finding, FindingClass, FindingKind, Lines, ReviewRoundOutcome, ReviewRoundRepository,
    ReviewRoundStart,
};

/// A new Run of Julia's, with the round that starts at Step 3 recorded and
/// its two reviewers' Steps, 3 and 4, started.
async fn reviewing(repo: &PgRunRepository) -> RunId {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(id, "code", "Add dark mode", &julia, None, None)
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };
    repo.start_review_round(id, &first_round())
        .await
        .expect("round");
    let entry = EntryId::try_from("review".to_string()).expect("entry");
    for seq in [3, 4] {
        repo.start_step(StepKey { run_id: id, seq }, &entry, 1)
            .await
            .expect("start");
    }
    id
}

fn first_round() -> ReviewRoundStart {
    ReviewRoundStart {
        seq: 3,
        round: 1,
        reviewers: 2,
        head_sha: "abc123".to_string(),
        diff_base: "main".to_string(),
        fix_diff: false,
    }
}

fn finding(path: &str, lines: Option<(u32, u32)>) -> Finding {
    Finding {
        path: path.to_string(),
        lines: lines.map(|(start, end)| Lines { start, end }),
        problem: format!("{path} is wrong"),
        why: "It breaks the build".to_string(),
    }
}

/// A round keeps its first record; a reviewer's findings replace an
/// earlier attempt's until its Step ends, and then stay as they are.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_round_keeps_its_first_record_and_the_findings_its_step_ended_with(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = reviewing(&repo).await;

    let again = ReviewRoundStart {
        head_sha: "def456".to_string(),
        ..first_round()
    };
    let round = repo.start_review_round(id, &again).await.expect("again");
    assert_eq!(round.head_sha, "abc123");
    assert_eq!(round.step_seqs(), 3..5);

    let key = StepKey { run_id: id, seq: 3 };
    let first = [finding("src/a.rs", Some((4, 9)))];
    assert!(repo.record_findings(key, 3, &first).await.expect("first"));
    let retried = [finding("src/b.rs", None), finding("src/c.rs", Some((1, 1)))];
    assert!(
        repo.record_findings(key, 3, &retried)
            .await
            .expect("retried")
    );

    repo.end_step(
        key,
        &Outcome::try_from("changes".to_string()).expect("outcome"),
        "Two",
    )
    .await
    .expect("end");
    let late = [finding("src/d.rs", None)];
    assert!(!repo.record_findings(key, 3, &late).await.expect("late"));

    let rounds = repo.review_rounds(id).await.expect("rounds");
    let paths: Vec<_> = rounds[0]
        .findings
        .iter()
        .map(|found| (found.seq, found.place, found.path.as_str(), found.lines))
        .collect();
    assert_eq!(
        paths,
        [
            (3, 1, "src/b.rs", None),
            (3, 2, "src/c.rs", Some(Lines { start: 1, end: 1 })),
        ]
    );
}

/// A finding keeps its first kind and its first task, and a round its
/// first end; a Blocker never gets a task.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "tasks"))
)]
async fn a_finding_keeps_its_first_kind_and_task_and_a_round_its_first_end(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = reviewing(&repo).await;
    let key = StepKey { run_id: id, seq: 4 };
    let found = [
        finding("src/a.rs", Some((2, 3))),
        finding("README.md", None),
    ];
    repo.record_findings(key, 3, &found).await.expect("record");

    let classes = [
        FindingClass {
            seq: 4,
            place: 1,
            kind: FindingKind::Blocker,
        },
        FindingClass {
            seq: 4,
            place: 2,
            kind: FindingKind::FollowUp,
        },
    ];
    repo.classify_findings(id, 3, &classes)
        .await
        .expect("classify");
    let flipped = [FindingClass {
        seq: 4,
        place: 1,
        kind: FindingKind::FollowUp,
    }];
    repo.classify_findings(id, 3, &flipped)
        .await
        .expect("again");

    let task = Uuid::parse_str("00000000-0000-7000-8000-000000000101").expect("task");
    let other = Uuid::parse_str("00000000-0000-7000-8000-000000000102").expect("task");
    repo.link_follow_up(key, 2, task).await.expect("link");
    repo.link_follow_up(key, 2, other).await.expect("relink");
    repo.link_follow_up(key, 1, other).await.expect("blocker");

    let ended = repo
        .end_review_round(id, 3, ReviewRoundOutcome::Changes)
        .await
        .expect("end")
        .expect("round");
    let again = repo
        .end_review_round(id, 3, ReviewRoundOutcome::Pass)
        .await
        .expect("again")
        .expect("round");
    assert_eq!(ended.end, again.end);
    assert_eq!(
        again.end.as_ref().map(|end| end.outcome),
        Some(ReviewRoundOutcome::Changes)
    );

    let kinds: Vec<_> = again
        .findings
        .iter()
        .map(|found| (found.place, found.kind, found.follow_up_task_id))
        .collect();
    assert_eq!(
        kinds,
        [
            (1, Some(FindingKind::Blocker), None),
            (2, Some(FindingKind::FollowUp), Some(task)),
        ]
    );
    assert_eq!(again.blockers().count(), 1);

    let listed = repo.runs_review_rounds(&[id]).await.expect("listed");
    assert_eq!(listed.get(&id), Some(&vec![again]));
}
