//! A Gate answered once, end to end on a running Temporal, through the Run
//! API its owner calls: the answer takes the Gate out of the owner's Queue
//! (`GET /runs/waiting`), is posted to the Run's task thread, marks the
//! Gate's notification done, and a second answer to it is refused (409).

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use factory_records::domain::models::{RunId, RunOutcome};
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules};
use macro_authorization::{
    AuthorizedUserApiKey, CredentialOrigin, InternalAuthConfig, JwtValidator,
    MacroAuthorizationError, MacroAuthorizationServiceImpl, MacroAuthorizationState,
    NoBotAuthorizer, UserApiKeyAuthorizer, ValidatedCredential, ValidatedIdentity,
};
use rootcause::Report;
use tower::ServiceExt as _;

use super::{CodeAgent, CodeRun, CodeWalk, PATIENCE, walk_code_with};
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::fakes::{FakeGithub, FakeNotifier, FakeRepo, FakeTaskThread, FakeWatches, ci};
use crate::domain::gate_owner::notice_id;
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::service::FactoryServiceImpl;
use crate::inbound::http::{FactoryRouterState, factory_router};
use crate::outbound::local_agent::LocalAgent;
use crate::outbound::temporal_run_workflows::TemporalRunWorkflows;

/// Accepts the bearer token `julia`: the Run's owner, in her own
/// signed-in session.
#[derive(Clone, Copy)]
struct JuliaSession;

impl JwtValidator for JuliaSession {
    fn validate_credential(
        &self,
        jwt: &str,
    ) -> Result<ValidatedCredential, Report<MacroAuthorizationError>> {
        if jwt != "julia" {
            return Err(Report::new(MacroAuthorizationError::InvalidCredentials));
        }
        Ok(ValidatedCredential {
            origin: CredentialOrigin::Session,
            identity: ValidatedIdentity {
                agent_id: None,
                agent_via: None,
                routine_id: None,
                user_id: super::julia().to_string(),
                fusion_user_id: "fusion-julia".to_string(),
                organization_id: None,
                permissions: None,
            },
        })
    }
}

/// Accepts no user API key.
#[derive(Clone, Copy)]
struct NoApiKeys;

impl UserApiKeyAuthorizer for NoApiKeys {
    async fn authorize_user_api_key(
        &self,
        _api_key: &str,
    ) -> Result<AuthorizedUserApiKey, Report<MacroAuthorizationError>> {
        Err(Report::new(MacroAuthorizationError::InvalidCredentials))
    }
}

/// The Run API over the records of `run`, answering through its Temporal.
fn run_api(run: &CodeRun, github: &FakeGithub) -> Router {
    let service = FactoryServiceImpl::new(
        run.repo.clone(),
        TemporalRunWorkflows::new(run.client.clone()),
        BuiltInSteps::new(
            LocalAgent,
            PullRequestSteps::new(github.clone(), run.repo.clone(), FakeWatches::default()),
        ),
        run.repo.clone(),
        run.repo.clone(),
        github.clone(),
    );
    let authorization = MacroAuthorizationState::new(Arc::new(MacroAuthorizationServiceImpl::new(
        JuliaSession,
        InternalAuthConfig {
            api_key: "test-internal-key".to_string(),
            default_user_id: None,
        },
        NoBotAuthorizer,
        NoApiKeys,
    )));
    factory_router(FactoryRouterState::new(Arc::new(service), authorization))
}

/// Call `path` on `api` as Julia, in her own session.
async fn as_julia(
    api: &Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", "Bearer julia")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = api.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    (status, json)
}

/// Whether `run_id`'s Gate at `seq` is in Julia's Queue: listed by
/// `GET /runs/waiting` with that Gate unanswered.
async fn queued(api: &Router, run_id: RunId, seq: u16) -> bool {
    let (status, waiting) = as_julia(api, "GET", "/runs/waiting", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{waiting}");
    let run_id = serde_json::to_value(run_id).expect("id");
    waiting["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .filter(|detail| detail["run"]["id"] == run_id)
        .flat_map(|detail| detail["steps"].as_array().cloned().unwrap_or_default())
        .any(|step| {
            step["seq"] == seq && step["gate"].is_object() && step["gate"]["answer"].is_null()
        })
}

/// Wait until `ready` holds.
async fn until(what: &str, mut ready: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(PATIENCE, async {
        while !ready().await {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

/// The factory's default rules, which fix Blockers, with the task thread
/// on.
fn posted() -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            task_thread: true,
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// Julia answers the plan Gate once through the Run API: it leaves her
/// Queue, its answer is posted to the Run's task thread, its notification
/// is marked done, and answering it again is refused as a conflict.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_gate_answered_once_leaves_the_queue_is_posted_and_is_done() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    let thread = FakeTaskThread::default();

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: posted(),
            notifier: Some(notifier.clone()),
            thread: Some(thread.clone()),
            ..CodeWalk::default()
        },
        |run| {
            let github = github.clone();
            let notifier = notifier.clone();
            let thread = thread.clone();
            async move {
                let api = run_api(&run, &github);
                let plan = run.next_gate().await;
                let told = notice_id(run.run_id, plan);
                // The Gate takes answers once its owner is told of it.
                until("the owner is told", async || {
                    notifier
                        .sent
                        .lock()
                        .expect("sent")
                        .iter()
                        .any(|notice| notice.notification_id == told)
                })
                .await;
                assert!(
                    queued(&api, run.run_id, plan).await,
                    "the gate waits on her"
                );
                assert!(!notifier.done.lock().expect("done").contains(&told));

                let answer = format!("/runs/{}/steps/{plan}/answer", run.run_id);
                let approve = serde_json::json!({ "answer": "approve" });
                let (status, body) = as_julia(&api, "POST", &answer, approve.clone()).await;
                assert_eq!(status, StatusCode::ACCEPTED, "{body}");

                until("the answer is recorded", async || {
                    run.gate(plan).await.answer.is_some()
                })
                .await;
                assert!(
                    !queued(&api, run.run_id, plan).await,
                    "an answered gate leaves her queue"
                );
                until("the notification is done", async || {
                    notifier.done.lock().expect("done").contains(&told)
                })
                .await;
                let posted =
                    format!("Step {plan}'s gate was answered `approve` by the run's owner.");
                until("the answer is posted", async || {
                    thread.texts().iter().any(|text| text.starts_with(&posted))
                })
                .await;

                let (again, body) = as_julia(&api, "POST", &answer, approve).await;
                assert_eq!(again, StatusCode::CONFLICT, "{body}");
                assert_eq!(
                    thread
                        .texts()
                        .iter()
                        .filter(|text| text.starts_with(&posted))
                        .count(),
                    1,
                    "the refused answer posts nothing"
                );

                let merge = run.next_gate().await;
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
}
