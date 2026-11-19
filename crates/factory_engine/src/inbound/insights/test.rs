use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use factory_records::domain::insights::{InsightsRecords, InsightsScope, RunSummary};
use factory_records::domain::models::{RunId, RunOutcome};
use macro_authorization::{
    AuthorizedUserApiKey, CredentialOrigin, INTERNAL_API_KEY_HEADER, INTERNAL_MACRO_USER_ID_HEADER,
    InternalAuthConfig, JwtValidator, MacroAuthorizationError, MacroAuthorizationServiceImpl,
    MacroAuthorizationState, NoBotAuthorizer, UserApiKeyAuthorizer, ValidatedCredential,
    ValidatedIdentity,
};
use macro_user_id::user_id::MacroUserIdStr;
use rootcause::Report;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{InsightsRouterState, insights_router};
use crate::domain::insights::fakes::{FakeRecords, FakeUsage};
use crate::domain::insights::{INSIGHTS_RUN_CAP, InsightsServiceImpl};

const KEY: &str = "test-internal-key";
const ALICE: &str = "macro|alice@example.com";
const EVE: &str = "macro|eve@example.com";

/// Accepts the bearer token `alice`, Alice's own session.
#[derive(Clone, Copy)]
struct Tokens;

impl JwtValidator for Tokens {
    fn validate_credential(
        &self,
        jwt: &str,
    ) -> Result<ValidatedCredential, Report<MacroAuthorizationError>> {
        if jwt != "alice" {
            return Err(Report::new(MacroAuthorizationError::InvalidCredentials));
        }
        Ok(ValidatedCredential {
            origin: CredentialOrigin::Session,
            identity: ValidatedIdentity {
                agent_id: None,
                agent_via: None,
                routine_id: None,
                user_id: ALICE.to_string(),
                fusion_user_id: "fusion-alice".to_string(),
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

/// The scopes the records were asked for.
type Asked = Arc<Mutex<Vec<InsightsScope>>>;

/// The insights router over records that hold one of Alice's Runs.
fn app() -> (Router, Asked) {
    let records = InsightsRecords {
        runs: vec![RunSummary {
            id: RunId::from_uuid(Uuid::from_u128(1)),
            title: "Plan the export".to_string(),
            flow: Some("plan".to_string()),
            created_at: "2026-11-10T09:00:00Z"
                .parse::<DateTime<Utc>>()
                .expect("valid time"),
            end: Some((
                RunOutcome::Success,
                "2026-11-10T09:30:00Z"
                    .parse::<DateTime<Utc>>()
                    .expect("valid time"),
            )),
        }],
        ..InsightsRecords::default()
    };
    let records = FakeRecords::new(records);
    let asked = records.asked.clone();
    let service = Arc::new(InsightsServiceImpl::new(
        records,
        FakeUsage::new(Vec::new()),
    ));
    let authorization = MacroAuthorizationState::new(Arc::new(MacroAuthorizationServiceImpl::new(
        Tokens,
        InternalAuthConfig {
            api_key: KEY.to_string(),
            default_user_id: None,
        },
        NoBotAuthorizer,
        NoApiKeys,
    )));
    let router = insights_router(InsightsRouterState::new(service, authorization));
    (router, asked)
}

fn get(path: &str) -> axum::http::request::Builder {
    Request::builder().method("GET").uri(path)
}

fn asked(asked: &Asked) -> Vec<InsightsScope> {
    asked.lock().expect("asked").clone()
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

const NOVEMBER: &str = "/insights?from=2026-11-01&to=2026-11-30&timeZone=UTC";

#[tokio::test]
async fn a_call_with_no_user_reads_nothing() {
    let (app, scopes) = app();

    let anonymous = app
        .clone()
        .oneshot(get(NOVEMBER).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let internal = app
        .oneshot(
            get(NOVEMBER)
                .header(INTERNAL_API_KEY_HEADER, KEY)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(internal.status(), StatusCode::UNAUTHORIZED);
    assert!(asked(&scopes).is_empty());
}

#[tokio::test]
async fn a_person_reads_their_own_runs_and_an_internal_call_its_acting_users() {
    let (app, scopes) = app();

    let alice = app
        .clone()
        .oneshot(
            get(NOVEMBER)
                .header("authorization", "Bearer alice")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let for_eve = app
        .oneshot(
            get(&format!("{NOVEMBER}&flow=plan"))
                .header(INTERNAL_API_KEY_HEADER, KEY)
                .header(INTERNAL_MACRO_USER_ID_HEADER, EVE)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(alice.status(), StatusCode::OK);
    assert_eq!(for_eve.status(), StatusCode::OK);
    let user = |id: &str| MacroUserIdStr::try_from(id.to_string()).expect("valid user id");
    let november = |user_id, flow: Option<&str>| InsightsScope {
        user_id,
        from: "2026-11-01T00:00:00Z".parse().expect("valid time"),
        to: "2026-12-01T00:00:00Z".parse().expect("valid time"),
        flow: flow.map(str::to_string),
        cap: INSIGHTS_RUN_CAP,
    };
    assert_eq!(
        asked(&scopes),
        vec![
            november(user(ALICE), None),
            november(user(EVE), Some("plan"))
        ]
    );
}

#[tokio::test]
async fn insights_answer_in_the_web_apps_shape() {
    let (app, _scopes) = app();

    let response = app
        .oneshot(
            get("/insights?from=2026-11-10&to=2026-11-11&timeZone=Europe/Lisbon")
                .header("authorization", "Bearer alice")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["from"], "2026-11-10");
    assert_eq!(body["timeZone"], "Europe/Lisbon");
    assert_eq!(body["runCap"], 1000);
    assert_eq!(body["capped"], false);
    assert_eq!(body["totals"]["success"], 1);
    assert_eq!(body["days"][0]["date"], "2026-11-10");
    assert_eq!(body["days"][0]["counts"]["success"], 1);
    assert_eq!(body["days"][1]["counts"]["success"], 0);
    assert_eq!(body["flows"][0]["flow"], "plan");
    assert_eq!(body["runDuration"]["medianSeconds"], 1800.0);
    assert_eq!(body["usage"]["agentSteps"], 0);
    assert_eq!(body["entries"], serde_json::json!([]));
}

#[tokio::test]
async fn a_bad_period_time_zone_or_flow_is_refused() {
    let (app, scopes) = app();
    let bad = [
        "/insights?from=2026-11-30&to=2026-11-01&timeZone=UTC",
        "/insights?from=2025-01-01&to=2026-11-01&timeZone=UTC",
        "/insights?from=2026-11-01&to=2026-11-30&timeZone=UTC&flow=Not%20a%20flow",
        "/insights?from=2026-11-01&timeZone=UTC",
    ];

    for path in bad {
        let response = app
            .clone()
            .oneshot(
                get(path)
                    .header("authorization", "Bearer alice")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    assert!(asked(&scopes).is_empty());
}

#[tokio::test]
async fn quality_answers_in_the_web_apps_shape_and_refuses_a_bad_repository() {
    let (app, _scopes) = app();

    let response = app
        .clone()
        .oneshot(
            get(
                "/insights/quality?from=2026-11-01&to=2026-11-30&timeZone=UTC&repository=macro/app",
            )
            .header("authorization", "Bearer alice")
            .body(Body::empty())
            .expect("request"),
        )
        .await
        .expect("response");
    let refused = app
        .oneshot(
            get("/insights/quality?from=2026-11-01&to=2026-11-30&timeZone=UTC&repository=macro")
                .header("authorization", "Bearer alice")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["repository"], "https://github.com/macro/app");
    assert_eq!(body["runCap"], 1000);
    assert_eq!(body["ci"]["causes"]["setup"], 0);
    assert_eq!(body["ci"]["recurring"], serde_json::json!([]));
    assert_eq!(body["review"]["cleanFirstRate"], serde_json::Value::Null);
    assert_eq!(body["delivery"]["autonomyRate"], serde_json::Value::Null);
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
}
