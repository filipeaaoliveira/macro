//! The Automations router: who may call each route, and what reaches the
//! service.

use std::sync::{Arc, Mutex};

use agent_correlation::context::{Attribution, scope_attribution};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use factory_records::domain::automations::Automation;
use macro_authorization::{
    AuthorizedUserApiKey, CredentialOrigin, InternalAuthConfig, JwtValidator,
    MacroAuthorizationError, MacroAuthorizationServiceImpl, MacroAuthorizationState,
    NoBotAuthorizer, UserApiKeyAuthorizer, ValidatedCredential, ValidatedIdentity,
};
use macro_user_id::user_id::MacroUserIdStr;
use rootcause::Report;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{AutomationsRouterState, automations_router};
use crate::domain::automations::{
    AutomationRequest, AutomationsSettings, CreatedAutomation, FiringResult, RoutineFiring,
};
use crate::domain::models::FactoryError;
use crate::domain::ports::AutomationService;

const ALICE: &str = "macro|alice@example.com";
const ROUTINE: Uuid = Uuid::from_u128(0x2007);

/// Accepts `alice`, her own session, and `sdk`, her Macro API token.
#[derive(Clone, Copy)]
struct Tokens;

impl JwtValidator for Tokens {
    fn validate_credential(
        &self,
        jwt: &str,
    ) -> Result<ValidatedCredential, Report<MacroAuthorizationError>> {
        let origin = match jwt {
            "alice" => CredentialOrigin::Session,
            "sdk" => CredentialOrigin::MacroApiJwt,
            _ => return Err(Report::new(MacroAuthorizationError::InvalidCredentials)),
        };
        Ok(ValidatedCredential {
            origin,
            identity: ValidatedIdentity {
                agent_id: None,
                agent_via: None,
                routine_id: None,
                user_id: ALICE.to_string(),
                fusion_user_id: format!("fusion-{jwt}"),
                organization_id: None,
                permissions: None,
            },
        })
    }
}

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

/// What each call reached the service with.
#[derive(Default)]
struct FakeAutomations {
    created_in_person: Mutex<Vec<bool>>,
    routine_firings: Mutex<Vec<(String, Uuid)>>,
    webhook_calls: Mutex<Vec<WebhookCall>>,
}

/// A webhook call: the Automation, timestamp, signature and body.
type WebhookCall = (Uuid, String, String, Vec<u8>);

fn fired() -> FiringResult {
    FiringResult {
        run_id: None,
        skipped: None,
        repeated: false,
    }
}

impl AutomationService for FakeAutomations {
    async fn automations(
        &self,
        _actor: &MacroUserIdStr<'static>,
    ) -> Result<AutomationsSettings, FactoryError> {
        Err(FactoryError::NoTeam)
    }

    async fn create_automation(
        &self,
        _actor: MacroUserIdStr<'static>,
        in_person: bool,
        _request: AutomationRequest,
    ) -> Result<CreatedAutomation, FactoryError> {
        self.created_in_person
            .lock()
            .expect("created")
            .push(in_person);
        Err(FactoryError::NoTeam)
    }

    async fn set_automation_enabled(
        &self,
        _actor: &MacroUserIdStr<'static>,
        _in_person: bool,
        _id: Uuid,
        _enabled: bool,
    ) -> Result<Automation, FactoryError> {
        Err(FactoryError::NotFound)
    }

    async fn delete_automation(
        &self,
        _actor: &MacroUserIdStr<'static>,
        _id: Uuid,
    ) -> Result<(), FactoryError> {
        Err(FactoryError::NotFound)
    }

    async fn fire_from_routine(
        &self,
        actor: &MacroUserIdStr<'static>,
        routine_id: Uuid,
        _firing: RoutineFiring,
    ) -> Result<FiringResult, FactoryError> {
        self.routine_firings
            .lock()
            .expect("firings")
            .push((actor.to_string(), routine_id));
        Ok(fired())
    }

    async fn fire_webhook(
        &self,
        id: Uuid,
        timestamp: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<FiringResult, FactoryError> {
        self.webhook_calls.lock().expect("calls").push((
            id,
            timestamp.to_string(),
            signature.to_string(),
            body.to_vec(),
        ));
        Ok(fired())
    }
}

fn router(service: &Arc<FakeAutomations>) -> Router {
    let authorization = MacroAuthorizationState::new(Arc::new(MacroAuthorizationServiceImpl::new(
        Tokens,
        InternalAuthConfig {
            api_key: "test-internal-key".to_string(),
            default_user_id: None,
        },
        NoBotAuthorizer,
        NoApiKeys,
    )));
    automations_router(AutomationsRouterState::new(service.clone(), authorization))
}

fn post(path: &str, token: Option<&str>, body: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request.body(Body::from(body.to_string())).expect("request")
}

const AUTOMATION: &str =
    r#"{"name":"Nightly","trigger":{"kind":"webhook"},"flow":"reply","requestTemplate":"Reply"}"#;
const FIRING: &str = r#"{"firing":"run-1"}"#;

/// Only the person in their own session makes an Automation in person; a
/// program with their token does not.
#[tokio::test]
async fn an_automation_is_made_in_person_only_from_a_session() {
    let service = Arc::new(FakeAutomations::default());

    router(&service)
        .oneshot(post("/automations", Some("alice"), AUTOMATION))
        .await
        .expect("response");
    router(&service)
        .oneshot(post("/automations", Some("sdk"), AUTOMATION))
        .await
        .expect("response");

    assert_eq!(
        *service.created_in_person.lock().expect("created"),
        vec![true, false]
    );
}

/// Only a call whose credential the scheduler signed for a routine fires
/// an Automation, and the routine is the one signed, never one the body
/// names.
#[tokio::test]
async fn only_a_routine_s_credential_fires_an_automation() {
    let service = Arc::new(FakeAutomations::default());

    let unsigned = router(&service)
        .oneshot(post("/automations/fire", Some("sdk"), FIRING))
        .await
        .expect("response");
    let signed = scope_attribution(
        Attribution {
            routine_id: Some(ROUTINE),
            ..Attribution::default()
        },
        router(&service).oneshot(post("/automations/fire", Some("sdk"), FIRING)),
    )
    .await
    .expect("response");

    assert_eq!(unsigned.status(), StatusCode::FORBIDDEN);
    assert_eq!(signed.status(), StatusCode::OK);
    assert_eq!(
        *service.routine_firings.lock().expect("firings"),
        vec![(ALICE.to_string(), ROUTINE)]
    );
}

/// A webhook call needs no login, but a call without its signature and
/// timestamp never reaches the service; one with them hands over the body
/// exactly as sent.
#[tokio::test]
async fn a_webhook_call_without_its_signature_is_refused() {
    let service = Arc::new(FakeAutomations::default());
    let id = Uuid::from_u128(0xa7);
    let path = format!("/automations/{id}/webhook");
    let body = r#"{"taskId":null}"#;

    let unsigned = router(&service)
        .oneshot(post(&path, None, body))
        .await
        .expect("response");
    let mut signed = post(&path, None, body);
    signed
        .headers_mut()
        .insert("x-macro-timestamp", "1763456400".parse().expect("header"));
    signed
        .headers_mut()
        .insert("x-macro-signature", "v1=abc".parse().expect("header"));
    let accepted = router(&service).oneshot(signed).await.expect("response");

    assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    assert_eq!(
        *service.webhook_calls.lock().expect("calls"),
        vec![(
            id,
            "1763456400".to_string(),
            "v1=abc".to_string(),
            body.as_bytes().to_vec()
        )]
    );
}
