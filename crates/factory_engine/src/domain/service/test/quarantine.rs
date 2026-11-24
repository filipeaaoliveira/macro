//! Quarantining a CI job by hand, and releasing it.

use chrono::Utc;
use factory_records::domain::models::TeamMembership;
use factory_records::domain::quality::{CiFailureIdentity, CiQuarantine};
use uuid::Uuid;

use super::{FakeWorkflows, code_service, julia, teo};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

const APP: &str = "https://github.com/Macro/App";
const APP_KEY: &str = "https://github.com/macro/app";

fn on_team(repo: &FakeRepo, admin: bool) {
    let team_id = Uuid::now_v7();
    repo.teams
        .lock()
        .expect("teams")
        .insert(julia().to_string(), TeamMembership { team_id, admin });
    repo.teams.lock().expect("teams").insert(
        teo().to_string(),
        TeamMembership {
            team_id,
            admin: false,
        },
    );
}

/// A quarantine a Run's reruns made of `job` failing in `step`.
fn by_reruns(job: &str, step: Option<&str>) -> CiQuarantine {
    CiQuarantine {
        repository: APP_KEY.to_string(),
        failure: CiFailureIdentity {
            name: job.to_string(),
            failed_step: step.map(str::to_string),
            failure: None,
        },
        run_id: None,
        passed_retries: 3,
        task_id: Uuid::now_v7(),
        filed: true,
        quarantined_at: Utc::now(),
        last_passed_at: Utc::now(),
    }
}

/// A team's owner or admin quarantines a whole job of a repository they may
/// push to, under its URL in lower case and its trimmed name, once: again
/// changes nothing. Releasing it lifts it with the quarantine its reruns
/// made beside it, and keeps the release; again changes nothing.
#[tokio::test]
async fn a_team_admin_quarantines_a_job_and_releases_it() {
    let repo = FakeRepo::default();
    on_team(&repo, true);
    let service = code_service(&repo, &FakeWorkflows::default(), &FakeGithub::default());
    repo.ci_quarantines
        .lock()
        .expect("quarantines")
        .push(by_reruns("lint", Some("Run lint")));

    let made = service
        .quarantine_ci_job(&julia(), APP, "  lint \n")
        .await
        .expect("quarantine");
    let again = service
        .quarantine_ci_job(&julia(), APP, "lint")
        .await
        .expect("quarantine again");

    assert_eq!(made.repository, APP_KEY);
    assert_eq!(made.job, "lint");
    assert!(made.changed);
    assert!(!again.changed);
    assert_eq!(repo.ci_quarantines.lock().expect("quarantines").len(), 2);
    assert_eq!(repo.ci_hand_quarantines.lock().expect("hand").len(), 1);

    let released = service
        .release_ci_job(&julia(), APP, "lint")
        .await
        .expect("release");
    let none = service
        .release_ci_job(&julia(), APP, "lint")
        .await
        .expect("release again");

    assert!(released.changed);
    assert!(!none.changed);
    assert!(repo.ci_quarantines.lock().expect("quarantines").is_empty());
    assert_eq!(
        repo.ci_releases.lock().expect("releases")[0],
        (APP_KEY.to_string(), "lint".to_string())
    );
}

/// A quarantine its reruns made of a failure that named no step and no
/// test becomes the hand's when a person quarantines the job: it is
/// changed, and covers the whole job from then on.
#[tokio::test]
async fn a_hand_quarantine_takes_over_an_unnamed_one_of_its_job() {
    let repo = FakeRepo::default();
    on_team(&repo, true);
    let service = code_service(&repo, &FakeWorkflows::default(), &FakeGithub::default());
    let unnamed = by_reruns("lint", None);
    let task_id = unnamed.task_id;
    repo.ci_quarantines
        .lock()
        .expect("quarantines")
        .push(unnamed);

    let made = service
        .quarantine_ci_job(&julia(), APP, "lint")
        .await
        .expect("quarantine");

    assert!(made.changed);
    assert_eq!(repo.ci_quarantines.lock().expect("quarantines").len(), 1);
    assert!(
        repo.ci_hand_quarantines
            .lock()
            .expect("hand")
            .contains(&task_id)
    );
}

/// A member who does not administer the team, someone on no team, an
/// agent's account, an admin who may not push to the repository or cannot
/// reach it, a repository that is no GitHub repository, and a job with no
/// name change nothing.
#[tokio::test]
async fn only_a_team_admin_who_may_push_changes_a_quarantine() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let no_team = code_service(&repo, &workflows, &FakeGithub::default())
        .quarantine_ci_job(&julia(), APP, "lint")
        .await;
    on_team(&repo, true);
    let service = code_service(&repo, &workflows, &FakeGithub::default());
    let member = service.quarantine_ci_job(&teo(), APP, "lint").await;
    let member_release = service.release_ci_job(&teo(), APP, "lint").await;
    let read_only = FakeGithub::default();
    read_only.state().read_only = true;
    let not_pushable = code_service(&repo, &workflows, &read_only)
        .quarantine_ci_job(&julia(), APP, "lint")
        .await;
    let unreachable = FakeGithub::default();
    unreachable.state().unreachable = true;
    let out_of_reach = code_service(&repo, &workflows, &unreachable)
        .release_ci_job(&julia(), APP, "lint")
        .await;
    let invalid = service
        .quarantine_ci_job(&julia(), "https://github.com/macro/app/tree/main", "lint")
        .await;
    let nameless = service.quarantine_ci_job(&julia(), APP, "  ").await;
    repo.agent_accounts
        .lock()
        .expect("agent accounts")
        .insert(julia().to_string());
    let agent = service.quarantine_ci_job(&julia(), APP, "lint").await;

    assert!(matches!(no_team, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(member, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(member_release, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(not_pushable, Err(FactoryError::NoPushAccess)));
    assert!(matches!(
        out_of_reach,
        Err(FactoryError::UnreachableRepository)
    ));
    assert!(matches!(invalid, Err(FactoryError::InvalidRepository)));
    assert!(matches!(nameless, Err(FactoryError::InvalidCiJob)));
    assert!(matches!(agent, Err(FactoryError::NotAPerson)));
    assert!(repo.ci_quarantines.lock().expect("quarantines").is_empty());
    assert!(repo.ci_releases.lock().expect("releases").is_empty());
}
