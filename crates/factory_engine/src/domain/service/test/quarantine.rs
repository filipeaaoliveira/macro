//! Quarantining a CI job by hand, and releasing it.

use factory_records::domain::models::TeamMembership;
use uuid::Uuid;

use super::{FakeWorkflows, code_service, julia, teo};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

const APP: &str = "https://github.com/Macro/App";

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

/// A team's owner or admin quarantines a job of a repository they may push
/// to, under its URL in lower case, once: again changes nothing. Releasing
/// it lifts it, with the quarantines its reruns made; again changes
/// nothing.
#[tokio::test]
async fn a_team_admin_quarantines_a_job_and_releases_it() {
    let repo = FakeRepo::default();
    on_team(&repo, true);
    let service = code_service(&repo, &FakeWorkflows::default(), &FakeGithub::default());

    let made = service
        .quarantine_ci_job(&julia(), APP, "lint")
        .await
        .expect("quarantine");
    let again = service
        .quarantine_ci_job(&julia(), APP, "lint")
        .await
        .expect("quarantine again");

    assert_eq!(made.repository, "https://github.com/macro/app");
    assert_eq!(made.job, "lint");
    assert!(made.changed);
    assert!(!again.changed);
    {
        let quarantines = repo.ci_quarantines.lock().expect("quarantines");
        assert_eq!(quarantines.len(), 1);
        assert_eq!(quarantines[0].repository, "https://github.com/macro/app");
        assert_eq!(quarantines[0].failure.name, "lint");
        assert_eq!(quarantines[0].failure.failed_step, None);
        assert_eq!(quarantines[0].passed_retries, 0);
    }

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
}

/// A member who does not administer the team, someone on no team, an
/// admin who may not push to the repository, and a job with no name change
/// nothing.
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
    let nameless = service.quarantine_ci_job(&julia(), APP, "").await;

    assert!(matches!(no_team, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(member, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(member_release, Err(FactoryError::NotATeamAdmin)));
    assert!(matches!(not_pushable, Err(FactoryError::NoPushAccess)));
    assert!(matches!(nameless, Err(FactoryError::InvalidCiJob { .. })));
    assert!(repo.ci_quarantines.lock().expect("quarantines").is_empty());
}
