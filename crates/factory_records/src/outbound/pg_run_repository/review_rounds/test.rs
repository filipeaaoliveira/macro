use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::models::{EntryId, NewRun, Outcome, RunId, RunInsert, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::review_rounds::{
    Finding, FindingClass, FindingKind, Lines, ReviewRoundFailure, ReviewRoundOutcome,
    ReviewRoundRepository, ReviewRoundStart,
};

/// A new Run of Julia's, with the round that starts at Step 3 recorded and
/// its two reviewers' Steps, 3 and 4, started.
async fn reviewing(repo: &PgRunRepository) -> RunId {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Add dark mode",
            user_id: &julia,
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
        })
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
        blocks_anywhere: false,
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
    assert_eq!(round.head_sha.as_deref(), Some("abc123"));
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
    assert_eq!(
        repo.review_rounds(id).await.expect("rounds")[0].answered,
        [3]
    );
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
/// first end; a Blocker never gets a task. A comment on a follow-up task
/// is marked on the round's findings linked to it, and on no other.
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
    let commented =
        |round_seq: u16, task_id: Uuid| repo.mark_follow_up_commented(id, round_seq, task_id);
    commented(3, other).await.expect("another task");
    commented(6, task).await.expect("another round");
    let before = repo.review_rounds(id).await.expect("rounds").remove(0);
    assert!(
        before
            .findings
            .iter()
            .all(|found| !found.follow_up_commented)
    );
    commented(3, task).await.expect("commented");
    commented(3, task).await.expect("again");

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
        .map(|found| {
            (
                found.place,
                found.kind,
                found.follow_up_task_id,
                found.follow_up_commented,
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (1, Some(FindingKind::Blocker), None, false),
            (2, Some(FindingKind::FollowUp), Some(task), true),
        ]
    );
    assert_eq!(again.blockers().count(), 1);

    let listed = repo.runs_review_rounds(&[id]).await.expect("listed");
    assert_eq!(listed.get(&id), Some(&vec![again]));
}

/// A round that could not open ends failed with why, and keeps its first
/// record. A reviewer's re-ask prompt and a follow-up's task id keep the
/// first an attempt recorded; a Blocker has no task id.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_failed_opening_a_re_ask_and_a_follow_up_key_keep_their_first_record(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = reviewing(&repo).await;
    let failure = ReviewRoundFailure {
        seq: 6,
        round: 2,
        reviewers: 2,
        failure: "The run's branch is not on GitHub".to_string(),
    };
    let failed = repo.fail_review_round(id, &failure).await.expect("fail");
    let again = ReviewRoundFailure {
        failure: "Another reason".to_string(),
        ..failure
    };
    repo.fail_review_round(id, &again).await.expect("again");
    let rounds = repo.review_rounds(id).await.expect("rounds");
    assert_eq!(rounds[1], failed);
    assert_eq!(failed.head_sha, None);
    assert_eq!(
        failed.failure.as_deref(),
        Some("The run's branch is not on GitHub")
    );
    assert_eq!(
        failed.end.map(|end| end.outcome),
        Some(ReviewRoundOutcome::Failed)
    );

    let key = StepKey { run_id: id, seq: 3 };
    let first = Uuid::now_v7();
    assert_eq!(repo.reask_action(key, 3, first).await.expect("ask"), first);
    assert_eq!(
        repo.reask_action(key, 3, Uuid::now_v7())
            .await
            .expect("again"),
        first
    );

    let found = [finding("src/a.rs", None), finding("docs/b.md", None)];
    repo.record_findings(key, 3, &found).await.expect("record");
    let classes = [
        FindingClass {
            seq: 3,
            place: 1,
            kind: FindingKind::Blocker,
        },
        FindingClass {
            seq: 3,
            place: 2,
            kind: FindingKind::FollowUp,
        },
    ];
    repo.classify_findings(id, 3, &classes)
        .await
        .expect("classify");
    let task = Uuid::now_v7();
    assert_eq!(
        repo.follow_up_key(key, 2, task).await.expect("key"),
        Some(task)
    );
    assert_eq!(
        repo.follow_up_key(key, 2, Uuid::now_v7())
            .await
            .expect("again"),
        Some(task)
    );
    assert_eq!(
        repo.follow_up_key(key, 1, Uuid::now_v7())
            .await
            .expect("blocker"),
        None
    );
}

/// The Run's rounds are numbered in order across the whole Run, whichever
/// visit of their entry each is: a round after a reset is the Run's next
/// round, and visit 1 again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn rounds_are_numbered_across_the_run_through_a_reset(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = reviewing(&repo).await;
    let after_reset = ReviewRoundStart {
        seq: 8,
        head_sha: "def456".to_string(),
        ..first_round()
    };
    let started = repo
        .start_review_round(id, &after_reset)
        .await
        .expect("round");

    assert_eq!((started.round, started.visit), (2, 1));
    let rounds = repo.review_rounds(id).await.expect("rounds");
    let numbered: Vec<(u16, u16, u16)> = rounds
        .iter()
        .map(|round| (round.seq, round.round, round.visit))
        .collect();
    assert_eq!(numbered, [(3, 1, 1), (8, 2, 1)]);
    let listed = repo.runs_review_rounds(&[id]).await.expect("listed");
    assert_eq!(listed.get(&id), Some(&rounds));
}
