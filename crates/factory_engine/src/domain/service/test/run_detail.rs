//! What a Run's own read shows beside its Steps: the flaky CI jobs its
//! `ci` Steps reran, the quarantines they made, and the child tasks its
//! `split` Step filed.

use chrono::Utc;
use factory_records::domain::models::{EntryId, StepKey};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::quality::{
    CiFailureIdentity, CiQuarantine, CiRetry, CiRetryMatch, CiRetryResult,
};
use uuid::Uuid;

use super::{FakeWorkflows, julia, julia_s_run, service};
use crate::domain::fakes::FakeRepo;
use crate::domain::ports::FactoryService;

const HEAD: &str = "1111111111111111111111111111111111111111";
const APP: &str = "https://github.com/macro/app";

/// A rerun of `name` that failed at `step`, ending `result`.
fn rerun(name: &str, step: &str, result: Option<CiRetryResult>) -> CiRetry {
    CiRetry {
        name: name.to_string(),
        head_sha: HEAD.to_string(),
        job_id: 7,
        workflow_run_id: Some(900),
        failed_step: Some(step.to_string()),
        failure: None,
        matched: CiRetryMatch::Record,
        result,
        requested_at: Utc::now(),
        settled_at: result.map(|_| Utc::now()),
    }
}

/// A Run's read lists its reruns by Step then job, each numbered among the
/// job's reruns, with how it ended: a passed rerun with the quarantine it
/// made, and a failed one. Another Run's reruns are not its own.
#[tokio::test]
async fn a_run_s_read_lists_its_passed_and_failed_reruns_and_their_quarantine() {
    let repo = FakeRepo::default();
    let run = julia_s_run(&repo).await;
    let other = julia_s_run(&repo).await;
    let first = StepKey {
        run_id: run,
        seq: 2,
    };
    let second = StepKey {
        run_id: run,
        seq: 4,
    };
    {
        let mut retries = repo.ci_retries.lock().expect("retries");
        retries.push((
            second,
            rerun("test", "Run tests", Some(CiRetryResult::Failed)),
        ));
        retries.push((
            first,
            rerun("test", "Run tests", Some(CiRetryResult::Passed)),
        ));
        retries.push((first, rerun("lint", "Lint", Some(CiRetryResult::Failed))));
        retries.push((
            StepKey {
                run_id: other,
                seq: 2,
            },
            rerun("test", "Run tests", None),
        ));
    }
    repo.ci_quarantines
        .lock()
        .expect("quarantines")
        .push(CiQuarantine {
            repository: APP.to_string(),
            failure: CiFailureIdentity {
                name: "test".to_string(),
                failed_step: Some("Run tests".to_string()),
                failure: None,
            },
            run_id: Some(run),
            passed_retries: 3,
            task_id: Uuid::from_u128(0xc01),
            filed: true,
            quarantined_at: Utc::now(),
            last_passed_at: Utc::now(),
        });

    let detail = service(&repo, &FakeWorkflows::default())
        .get_run(&julia(), run)
        .await
        .expect("read");

    let reruns: Vec<(u16, &str, u32, Option<CiRetryResult>)> = detail
        .ci_reruns
        .iter()
        .map(|rerun| (rerun.seq, rerun.job.as_str(), rerun.attempt, rerun.result))
        .collect();
    assert_eq!(
        reruns,
        [
            (2, "lint", 1, Some(CiRetryResult::Failed)),
            (2, "test", 1, Some(CiRetryResult::Passed)),
            (4, "test", 2, Some(CiRetryResult::Failed)),
        ]
    );
    assert_eq!(detail.ci_reruns[1].step.as_deref(), Some("Run tests"));
    assert!(detail.ci_reruns[1].settled_at.is_some());
    assert_eq!(detail.ci_quarantines.len(), 1);
    assert_eq!(detail.ci_quarantines[0].job, "test");
    assert_eq!(detail.ci_quarantines[0].passed_retries, 3);
    assert_eq!(
        detail.ci_quarantines[0].task_id,
        Some(Uuid::from_u128(0xc01))
    );
}

/// A Run's read lists the child tasks its `split` Step filed, by place,
/// with their ids and names, from the records alone.
#[tokio::test]
async fn a_run_s_read_lists_the_child_tasks_its_split_filed() {
    let repo = FakeRepo::default();
    let run = julia_s_run(&repo).await;
    let split = StepKey {
        run_id: run,
        seq: 3,
    };
    repo.start_step(
        split,
        &EntryId::try_from("split".to_string()).expect("entry"),
        1,
    )
    .await
    .expect("start");
    repo.record_split_task(split, 1, Uuid::from_u128(0x7002), "Move the schema")
        .await
        .expect("record");
    repo.record_split_task(split, 0, Uuid::from_u128(0x7001), "Rebuild the toolbar")
        .await
        .expect("record");

    let detail = service(&repo, &FakeWorkflows::default())
        .get_run(&julia(), run)
        .await
        .expect("read");

    let children: Vec<(u16, u16, Uuid, &str)> = detail
        .split_tasks
        .iter()
        .map(|task| (task.seq, task.place, task.task_id, task.name.as_str()))
        .collect();
    assert_eq!(
        children,
        [
            (3, 0, Uuid::from_u128(0x7001), "Rebuild the toolbar"),
            (3, 1, Uuid::from_u128(0x7002), "Move the schema"),
        ]
    );
}
