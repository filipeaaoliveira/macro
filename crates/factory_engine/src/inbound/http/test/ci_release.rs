//! `POST /insights/ci-quarantines/release`: releasing a CI job.

use axum::http::StatusCode;
use factory_records::domain::models::TeamMembership;
use serde_json::json;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{ALICE, Caller, body, call, github_router, records_router};
use crate::domain::fakes::{FakeGithub, FakeRepo};

const PATH: &str = "/insights/ci-quarantines/release";

fn alice_admin(repo: &FakeRepo) {
    repo.teams.lock().expect("teams").insert(
        ALICE.to_string(),
        TeamMembership {
            team_id: Uuid::now_v7(),
            admin: true,
        },
    );
}

/// Only a team admin in their own signed-in session may: an internal caller
/// acting for them, their API key, an agent working as them and a person on
/// no team are refused 403, and nothing changes.
#[tokio::test]
async fn only_an_admin_in_person_may() {
    let repo = FakeRepo::default();
    alice_admin(&repo);
    let router = records_router(&repo);
    let job = json!({ "repository": "https://github.com/macro/app", "job": "lint" });

    for caller in [
        Caller::Internal(Some(ALICE)),
        Caller::ApiKey("alice-key"),
        Caller::User("agent"),
        Caller::User("eve"),
    ] {
        let response = router
            .clone()
            .oneshot(call("POST", PATH, caller, job.clone()))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    assert!(repo.ci_releases.lock().expect("releases").is_empty());
    assert!(repo.ci_quarantines.lock().expect("quarantines").is_empty());

    let person = router
        .oneshot(call("POST", PATH, Caller::User("alice"), job))
        .await
        .expect("response");
    assert_eq!(person.status(), StatusCode::OK);
    assert_eq!(
        body(person).await,
        json!({ "repository": "https://github.com/macro/app", "job": "lint", "changed": false })
    );
}

/// A repository that is no GitHub repository, one the admin may not push
/// to, and a job with no name are refused 400.
#[tokio::test]
async fn a_bad_repository_or_job_is_refused() {
    let repo = FakeRepo::default();
    alice_admin(&repo);
    let read_only = FakeGithub::default();
    read_only.state().read_only = true;
    let send = |router: axum::Router, repository: &str, job: &str| {
        let request = call(
            "POST",
            PATH,
            Caller::User("alice"),
            json!({ "repository": repository, "job": job }),
        );
        async move { router.oneshot(request).await.expect("response").status() }
    };

    let invalid = send(
        records_router(&repo),
        "https://github.com/macro/app/tree/main",
        "lint",
    )
    .await;
    let nameless = send(records_router(&repo), "https://github.com/macro/app", " ").await;
    let not_pushable = send(
        github_router(&repo, read_only),
        "https://github.com/macro/app",
        "lint",
    )
    .await;

    for refused in [invalid, nameless, not_pushable] {
        assert_eq!(refused, StatusCode::BAD_REQUEST);
    }
    assert!(repo.ci_releases.lock().expect("releases").is_empty());
    assert!(repo.ci_quarantines.lock().expect("quarantines").is_empty());
}
