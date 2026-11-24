use super::*;
use crate::domain::routines::RoutinePendingReason;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use bot_id::BotId;
use macro_authorization::{
    BOT_SCOPE_HEADER, BOT_TOKEN_HEADER, BotActingUserClaims, BotAuthentication, BotAuthorizer,
    BotScope, HARNESS_TOKEN_HEADER, HarnessAuthentication, HarnessAuthorizationOwner,
    HarnessAuthorizer, INTERNAL_API_KEY_HEADER, InternalAuthConfig, JwtValidator,
    MacroAuthorizationError, MacroAuthorizationServiceImpl, MacroUserAuthentication,
    NoUserApiKeyAuthorizer, ValidatedIdentity,
};
use macro_user_id::user_id::MacroUserIdStr;
use rootcause::Report;
use serde_json::{Value, json};
use std::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

const OWNER: &str = "macro|owner@example.com";
const INTERNAL_KEY: &str = "routine-test-internal";
const OPERATIONS: [&str; 11] = [
    "validate",
    "prepare",
    "prompt",
    "follow-up",
    "exec",
    "status",
    "cancel",
    "delete",
    "activity",
    "reply",
    "usage",
];

#[derive(Clone)]
struct Credentials;

impl JwtValidator for Credentials {
    fn validate_credential(
        &self,
        _: &str,
    ) -> Result<macro_authorization::ValidatedCredential, Report<MacroAuthorizationError>> {
        Ok(macro_authorization::ValidatedCredential {
            origin: macro_authorization::CredentialOrigin::Session,
            identity: ValidatedIdentity {
                agent_id: None,
                agent_via: None,
                routine_id: None,
                user_id: OWNER.into(),
                fusion_user_id: "fusion-owner".into(),
                organization_id: None,
                permissions: None,
            },
        })
    }
}

impl BotAuthorizer for Credentials {
    async fn authorize_bot(
        &self,
        _: &str,
        bot_scope: BotScope,
        _: Option<BotActingUserClaims>,
    ) -> Result<BotAuthentication, Report<MacroAuthorizationError>> {
        Ok(BotAuthentication {
            bot_id: BotId::TEST_A,
            token_id: Uuid::now_v7(),
            bot_scope,
            team_id: None,
            acting_user: None,
        })
    }
}

impl HarnessAuthorizer for Credentials {
    async fn authorize_harness(
        &self,
        _: &str,
        _: Option<String>,
    ) -> Result<HarnessAuthentication, Report<MacroAuthorizationError>> {
        Ok(HarnessAuthentication {
            harness_id: harness_id::HarnessId::TEST_A,
            token_id: Uuid::now_v7(),
            owner: HarnessAuthorizationOwner::User {
                user_id: OWNER.into(),
            },
            acting_user: MacroUserAuthentication {
                origin: macro_authorization::CredentialOrigin::Harness,
                agent_id: None,
                agent_via: None,
                routine_id: None,
                macro_user_id: MacroUserIdStr::try_from(OWNER).unwrap(),
                user_context: model_user::UserContext {
                    user_id: OWNER.into(),
                    fusion_user_id: "fusion-owner".into(),
                    organization_id: None,
                    permissions: None,
                },
            },
        })
    }
}

#[derive(Default)]
struct ServiceSpy {
    seen: Mutex<Vec<(&'static str, Value)>>,
    error: Option<RoutineSessionError>,
}

impl ServiceSpy {
    fn record(
        &self,
        operation: &'static str,
        command: impl serde::Serialize,
    ) -> Result<(), RoutineSessionError> {
        self.seen
            .lock()
            .unwrap()
            .push((operation, serde_json::to_value(command).unwrap()));
        self.error.map_or(Ok(()), Err)
    }
}

impl RoutineSessions for ServiceSpy {
    async fn validate(
        &self,
        command: ValidateRoutineSession,
    ) -> Result<ValidatedRoutineSession, RoutineSessionError> {
        self.record("validate", command)?;
        Ok(ValidatedRoutineSession { managed: true })
    }

    async fn prepare(
        &self,
        command: PrepareRoutineSession,
    ) -> Result<PreparedRoutineSession, RoutineSessionError> {
        let session_id = command.session_id;
        self.record("prepare", command)?;
        Ok(PreparedRoutineSession { session_id })
    }

    async fn prompt(
        &self,
        command: PromptRoutineSession,
    ) -> Result<RoutinePromptAccepted, RoutineSessionError> {
        let action_id = command.action.action_id;
        self.record("prompt", command)?;
        Ok(RoutinePromptAccepted {
            action_id,
            queued: true,
        })
    }

    async fn follow_up(
        &self,
        command: PromptRoutineSession,
    ) -> Result<RoutinePromptAccepted, RoutineSessionError> {
        let action_id = command.action.action_id;
        self.record("follow-up", command)?;
        Ok(RoutinePromptAccepted {
            action_id,
            queued: false,
        })
    }

    async fn exec(
        &self,
        command: ExecRoutineSession,
    ) -> Result<SessionCommandOutput, RoutineSessionError> {
        self.record("exec", command)?;
        Ok(SessionCommandOutput {
            exit_code: 1,
            stdout: "1 failed".into(),
            stderr: String::new(),
        })
    }

    async fn status(
        &self,
        command: RoutineSessionAction,
    ) -> Result<RoutineActionStatus, RoutineSessionError> {
        self.record("status", command)?;
        Ok(RoutineActionStatus::Pending(
            RoutinePendingReason::Permission,
        ))
    }

    async fn cancel(&self, command: RoutineSessionAction) -> Result<(), RoutineSessionError> {
        self.record("cancel", command)
    }

    async fn delete(&self, command: RoutineSessionAction) -> Result<(), RoutineSessionError> {
        self.record("delete", command)
    }

    async fn activity(
        &self,
        command: RoutineSessionAction,
    ) -> Result<crate::domain::routines::RoutineActivity, RoutineSessionError> {
        self.record("activity", command)?;
        Ok(crate::domain::routines::RoutineActivity {
            tool_calls: 3,
            turns: 1,
        })
    }

    async fn reply(
        &self,
        command: RoutineSessionAction,
    ) -> Result<Option<String>, RoutineSessionError> {
        self.record("reply", command)?;
        Ok(Some("I stopped before the tests.".to_string()))
    }

    async fn usage(
        &self,
        command: RoutineSessionAction,
    ) -> Result<Option<crate::domain::routines::RoutineUsage>, RoutineSessionError> {
        self.record("usage", command)?;
        Ok(Some(crate::domain::routines::RoutineUsage {
            input_tokens: 6,
            output_tokens: 1165,
            cache_read_tokens: 89_953,
            cache_write_tokens: 2845,
            cost_usd: Some(0.27),
            model: Some("claude-sonnet-5-5".to_string()),
        }))
    }
}

fn app(service: Arc<ServiceSpy>) -> Router {
    let auth = MacroAuthorizationServiceImpl::new(
        Credentials,
        InternalAuthConfig {
            api_key: INTERNAL_KEY.into(),
            default_user_id: None,
        },
        Credentials,
        NoUserApiKeyAuthorizer,
    )
    .with_harness_authorizer(Credentials);
    routine_sessions_router(RoutineSessionsState::new(
        service,
        MacroAuthorizationState::new(Arc::new(auth)),
    ))
}

fn command(operation: &str) -> Value {
    let selection = json!({ "owner": OWNER, "bot_id": BotId::TEST_A, "model": "chosen-model" });
    let action = json!({
        "owner": OWNER, "bot_id": BotId::TEST_A,
        "session_id": Uuid::now_v7(), "action_id": Uuid::now_v7(),
    });
    match operation {
        "validate" => selection,
        "prepare" => json!({
            "selection": selection, "session_id": Uuid::now_v7(),
            "source": "routine", "visibility": "unlisted",
        }),
        "prompt" | "follow-up" => {
            json!({ "action": action, "prompt": "private routine instructions" })
        }
        "exec" => json!({
            "action": action,
            "command": { "cwd": ".", "command": "just check", "timeout_secs": 600 },
        }),
        _ => action,
    }
}

fn request(operation: &str, body: Value, headers: &[(&str, &str)]) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/internal/routine-sessions/{operation}"))
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn every_operation_requires_internal_credentials_not_user_bot_or_harness() {
    let service = Arc::new(ServiceSpy::default());
    for operation in OPERATIONS {
        for (headers, expected) in [
            (vec![], StatusCode::UNAUTHORIZED),
            (
                vec![(INTERNAL_API_KEY_HEADER, "wrong")],
                StatusCode::UNAUTHORIZED,
            ),
            (
                vec![("authorization", "Bearer valid-user")],
                StatusCode::FORBIDDEN,
            ),
            (
                vec![(BOT_TOKEN_HEADER, "mbot_valid"), (BOT_SCOPE_HEADER, "user")],
                StatusCode::FORBIDDEN,
            ),
            (
                vec![(HARNESS_TOKEN_HEADER, "mhns_valid")],
                StatusCode::FORBIDDEN,
            ),
        ] {
            let response = app(service.clone())
                .oneshot(request(operation, command(operation), &headers))
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{operation}: {headers:?}");
        }
    }
    assert!(service.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forwards_explicit_owner_and_complete_command_without_rewriting() {
    let service = Arc::new(ServiceSpy::default());
    for operation in OPERATIONS {
        let command = command(operation);
        let response = app(service.clone())
            .oneshot(request(
                operation,
                command.clone(),
                &[(INTERNAL_API_KEY_HEADER, INTERNAL_KEY)],
            ))
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(
            service.seen.lock().unwrap().last(),
            Some(&(operation, command))
        );
        if operation == "status" {
            assert_eq!(
                body(response).await,
                json!({ "state": "pending", "reason": "permission" })
            );
        } else if operation == "exec" {
            assert_eq!(
                body(response).await,
                json!({ "exit_code": 1, "stdout": "1 failed", "stderr": "" })
            );
        } else if operation == "reply" {
            assert_eq!(body(response).await, json!("I stopped before the tests."));
        } else if operation == "usage" {
            assert_eq!(
                body(response).await,
                json!({
                    "input_tokens": 6, "output_tokens": 1165,
                    "cache_read_tokens": 89_953, "cache_write_tokens": 2845,
                    "cost_usd": 0.27, "model": "claude-sonnet-5-5",
                })
            );
        }
    }
}

#[tokio::test]
async fn owner_is_required_and_validated_in_every_command() {
    let service = Arc::new(ServiceSpy::default());
    for operation in OPERATIONS {
        for invalid in [None, Some(json!("invalid-owner")), Some(Value::Null)] {
            let mut command = command(operation);
            let identity = match operation {
                "prepare" => &mut command["selection"],
                "prompt" | "follow-up" | "exec" => &mut command["action"],
                _ => &mut command,
            }
            .as_object_mut()
            .unwrap();
            identity.remove("owner");
            if let Some(invalid) = invalid {
                identity.insert("owner".into(), invalid);
            }
            let response = app(service.clone())
                .oneshot(request(
                    operation,
                    command,
                    &[(INTERNAL_API_KEY_HEADER, INTERNAL_KEY)],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{operation}");
            assert_eq!(body(response).await, json!({ "code": "invalid_command" }));
        }
    }
    assert!(service.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn domain_denials_are_preserved_as_content_free_error_codes() {
    for (error, status) in [
        (
            RoutineSessionError::Admission(ai_billing::AiAdmissionError::Denied(
                ai_billing::DenyReason::AllowanceExhausted,
            )),
            StatusCode::PAYMENT_REQUIRED,
        ),
        (
            RoutineSessionError::Admission(ai_billing::AiAdmissionError::Unavailable),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (RoutineSessionError::InvalidCommand, StatusCode::BAD_REQUEST),
        (RoutineSessionError::Forbidden, StatusCode::FORBIDDEN),
        (
            RoutineSessionError::PersonaUnavailable,
            StatusCode::NOT_FOUND,
        ),
        (
            RoutineSessionError::RuntimeUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (RoutineSessionError::SessionMismatch, StatusCode::CONFLICT),
        (RoutineSessionError::ModelMismatch, StatusCode::CONFLICT),
        (RoutineSessionError::Conflict, StatusCode::CONFLICT),
        (
            RoutineSessionError::CommandUnsupported,
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            RoutineSessionError::PromptDeliveryUnknown,
            StatusCode::BAD_GATEWAY,
        ),
        (
            RoutineSessionError::OperationFailed,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        for operation in OPERATIONS {
            let service = Arc::new(ServiceSpy {
                error: Some(error),
                ..Default::default()
            });
            let response = app(service.clone())
                .oneshot(request(
                    operation,
                    command(operation),
                    &[(INTERNAL_API_KEY_HEADER, INTERNAL_KEY)],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(body(response).await, json!({ "code": error }));
            assert_eq!(service.seen.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn oversized_prompt_is_rejected_without_dispatch_or_content_in_error() {
    let service = Arc::new(ServiceSpy::default());
    let mut command = command("prompt");
    command["prompt"] = json!("x".repeat(MAX_COMMAND_BYTES));
    let response = app(service.clone())
        .oneshot(request(
            "prompt",
            command,
            &[(INTERNAL_API_KEY_HEADER, INTERNAL_KEY)],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(response).await, json!({ "code": "invalid_command" }));
    assert!(service.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn bounded_request_timeout_never_invites_prompt_replay() {
    for operation in OPERATIONS {
        let response = bounded_response(
            operation == "prompt",
            Duration::ZERO,
            std::future::pending::<Response>(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let code = if operation == "prompt" {
            "prompt_delivery_unknown"
        } else {
            "operation_failed"
        };
        assert_eq!(body(response).await, json!({ "code": code }));
    }
}
