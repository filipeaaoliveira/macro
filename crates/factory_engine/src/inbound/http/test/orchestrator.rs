//! The Orchestrator through the Run API: an agent session in Macro chat
//! answers Gates for the person from its code, with their own access, and
//! is refused what the factory keeps for them in person; and a Run refused
//! at the Queue limit says why.

use agent_correlation::context::{Attribution, scope_attribution};
use agent_correlation::scope::SessionScope;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use bot_id::BotId;
use factory_records::domain::models::{EntryId, Outcome, StepKey, TeamMembership};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_acts::{StandingAct, StandingRoleName};
use factory_records::domain::standing_rules::OrchestratorDefinition;
use macro_authorization::CODE_EXECUTION_HEADER;
use macro_user_id::user_id::MacroUserIdStr;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{ALICE, Caller, alice_s_run, body, call, records_router, router};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::{ActingAgent, Answerer, FactoryError};

/// A request made from code in the agent session `session_id`, as the
/// correlation layer accepts it.
async fn in_session(
    router: &Router,
    session_id: Uuid,
    request: Request<Body>,
) -> axum::response::Response {
    let attribution = Attribution {
        session_id: Some(session_id),
        ..Attribution::default()
    };
    in_attribution(router, attribution, request).await
}

/// A request handled with `attribution` accepted at its boundary.
async fn in_attribution(
    router: &Router,
    attribution: Attribution,
    request: Request<Body>,
) -> axum::response::Response {
    scope_attribution(attribution, router.clone().oneshot(request))
        .await
        .expect("response")
}

/// A code-mode call: an agent's token, with code mode's header.
fn from_code(method: &str, path: &str, body: serde_json::Value) -> Request<Body> {
    let mut request = call(method, path, Caller::User("agent"), body);
    request
        .headers_mut()
        .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
    request
}

/// A code-mode call from an agent session reaches the service as an agent
/// answering for the person; the person in their own session as
/// themselves. Code with no agent session, and an internal caller even in
/// one, answer nothing; nor does an agent session reply in words, which
/// stays the person's.
#[tokio::test]
async fn an_agent_session_answers_for_the_person_and_only_it() {
    let (router, service) = router(|| FactoryError::NotFound);
    let run = factory_records::domain::models::RunId::generate();
    let answer_path = format!("/runs/{run}/steps/1/answer");
    let reply_path = format!("/runs/{run}/steps/1/reply");
    let answer = serde_json::json!({ "answer": "approve" });
    let session_id = Uuid::now_v7();

    let agent = in_session(
        &router,
        session_id,
        from_code("POST", &answer_path, answer.clone()),
    )
    .await;
    let person = router
        .clone()
        .oneshot(call(
            "POST",
            &answer_path,
            Caller::User("alice"),
            answer.clone(),
        ))
        .await
        .expect("response");
    let sessionless = router
        .clone()
        .oneshot(from_code("POST", &answer_path, answer.clone()))
        .await
        .expect("response");
    let internal = in_session(
        &router,
        session_id,
        call(
            "POST",
            &answer_path,
            Caller::Internal(Some(ALICE)),
            answer.clone(),
        ),
    )
    .await;
    let reply = in_session(
        &router,
        session_id,
        from_code(
            "POST",
            &reply_path,
            serde_json::json!({ "reply": "Looks right" }),
        ),
    )
    .await;

    // A routine's run is not a chat: signed for the routine, carrying its
    // id, or a background run held to a scope.
    let mut signed = call(
        "POST",
        &answer_path,
        Caller::User("routine"),
        answer.clone(),
    );
    signed
        .headers_mut()
        .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
    let signed = in_session(&router, session_id, signed).await;
    let carried = in_attribution(
        &router,
        Attribution {
            session_id: Some(session_id),
            routine_id: Some(Uuid::now_v7()),
            ..Attribution::default()
        },
        from_code("POST", &answer_path, answer.clone()),
    )
    .await;
    let scoped = in_attribution(
        &router,
        Attribution {
            session_id: Some(session_id),
            scope: Some(SessionScope::NoSend),
            ..Attribution::default()
        },
        from_code("POST", &answer_path, answer.clone()),
    )
    .await;
    for routine in [signed, carried, scoped] {
        assert_eq!(routine.status(), StatusCode::FORBIDDEN);
        assert_eq!(body(routine).await["code"], "reservedForAPerson");
    }

    assert_eq!(agent.status(), StatusCode::NOT_FOUND);
    assert_eq!(person.status(), StatusCode::NOT_FOUND);
    assert_eq!(sessionless.status(), StatusCode::FORBIDDEN);
    assert_eq!(internal.status(), StatusCode::FORBIDDEN);
    assert_eq!(reply.status(), StatusCode::FORBIDDEN);
    assert_eq!(*service.actors.lock().expect("actors"), [ALICE, ALICE]);
    assert_eq!(
        *service.answerers.lock().expect("answerers"),
        [Answerer::Agent { session_id }, Answerer::InPerson]
    );
}

/// A start, a cancel and a link from code in an agent session reach the
/// service as that session, for its Standing Roles to check; a refusal
/// says which Standing Roles allow the act. The person in their own
/// session, and an internal caller even in a session, reach it as no
/// agent.
#[tokio::test]
async fn starts_cancels_and_links_carry_the_calling_agent_session() {
    let (router, service) = router(|| FactoryError::NotInStandingRole {
        act: StandingAct::Cancel,
        allowing: vec![StandingRoleName::try_from("operator".to_string()).expect("valid")],
    });
    let run = factory_records::domain::models::RunId::generate();
    let start = serde_json::json!({ "flow": "reply", "request": "Say hello" });
    let link = serde_json::json!({ "taskId": Uuid::now_v7() });
    let cancel_path = format!("/runs/{run}/cancel");
    let link_path = format!("/runs/{run}/task");
    let session_id = Uuid::now_v7();

    let mut refusals = Vec::new();
    for (method, path, request) in [
        ("POST", "/runs", start.clone()),
        ("POST", cancel_path.as_str(), serde_json::json!({})),
        ("PUT", link_path.as_str(), link.clone()),
    ] {
        refusals.push(in_session(&router, session_id, from_code(method, path, request)).await);
    }
    let person = router
        .clone()
        .oneshot(call(
            "POST",
            &cancel_path,
            Caller::User("alice"),
            serde_json::json!({}),
        ))
        .await
        .expect("response");
    let internal = in_session(
        &router,
        session_id,
        call("POST", "/runs", Caller::Internal(Some(ALICE)), start),
    )
    .await;

    for refusal in refusals {
        assert_eq!(refusal.status(), StatusCode::FORBIDDEN);
        let body = body(refusal).await;
        assert_eq!(body["code"], "notInStandingRole");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("operator"))
        );
    }
    for other in [person, internal] {
        assert_eq!(other.status(), StatusCode::FORBIDDEN);
    }
    let session = Some(ActingAgent::Session(session_id));
    assert_eq!(
        *service.acting.lock().expect("acting"),
        [session, session, session, None, None]
    );
}

/// Code signed for a bot that runs in no agent session acts as that bot:
/// a self-hosted agent's code-mode call, and a routine's run signed for an
/// agent, reach the service with the bot their credential is signed for,
/// so Standing Roles check it. Code signed only for the person, their API
/// key and a routine running as them with no agent, is the person acting
/// and reaches it as no agent.
#[tokio::test]
async fn sessionless_code_signed_for_a_bot_acts_as_that_bot() {
    let (router, service) = router(|| FactoryError::NotFound);
    let run = factory_records::domain::models::RunId::generate();
    let cancel_path = format!("/runs/{run}/cancel");
    let link_path = format!("/runs/{run}/task");
    let start = serde_json::json!({ "flow": "reply", "request": "Say hello" });
    let link = serde_json::json!({ "taskId": Uuid::now_v7() });

    for request in [
        from_code("POST", "/runs", start.clone()),
        from_code("POST", &cancel_path, serde_json::json!({})),
        from_code("PUT", &link_path, link.clone()),
        call(
            "POST",
            &cancel_path,
            Caller::User("routine-agent"),
            serde_json::json!({}),
        ),
        call(
            "POST",
            &cancel_path,
            Caller::User("routine"),
            serde_json::json!({}),
        ),
        call(
            "POST",
            &cancel_path,
            Caller::ApiKey("alice-key"),
            serde_json::json!({}),
        ),
    ] {
        let response = router.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let bot = Some(ActingAgent::Bot(BotId::new_from_uuid(Uuid::from_u128(
        0xB07A,
    ))));
    assert_eq!(
        *service.acting.lock().expect("acting"),
        [bot, bot, bot, bot, None, None]
    );
}

/// Public MCP signs the person's own MCP client and API key for the Macro
/// bot only to label their posts: code from either in no session, through
/// MCP code mode or not, is the person acting and reaches the service as
/// no agent. The same credential in an agent session acts as that session.
#[tokio::test]
async fn the_person_s_own_mcp_client_acts_as_the_person() {
    let (router, service) = router(|| FactoryError::NotFound);
    let run = factory_records::domain::models::RunId::generate();
    let cancel_path = format!("/runs/{run}/cancel");
    let session_id = Uuid::now_v7();
    let mcp = |token: &'static str, code_mode: bool| {
        let mut request = call(
            "POST",
            &cancel_path,
            Caller::User(token),
            serde_json::json!({}),
        );
        if code_mode {
            request
                .headers_mut()
                .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
        }
        request
    };

    for request in [
        mcp("mcp-client", true),
        mcp("mcp-key", true),
        mcp("mcp-client", false),
        mcp("mcp-key", false),
        call(
            "POST",
            "/runs",
            Caller::User("mcp-client"),
            serde_json::json!({ "flow": "reply", "request": "Say hello" }),
        ),
    ] {
        let response = router.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let in_a_session = in_session(&router, session_id, mcp("mcp-client", true)).await;
    assert_eq!(in_a_session.status(), StatusCode::NOT_FOUND);

    assert_eq!(
        *service.acting.lock().expect("acting"),
        [
            None,
            None,
            None,
            None,
            None,
            Some(ActingAgent::Session(session_id))
        ]
    );
}

/// An agent's question is the person's to answer: an agent session's
/// answer is refused with a code it acts on, and the reason in words.
#[tokio::test]
async fn an_agent_s_answer_to_a_question_says_it_is_the_person_s() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);
    let run_id = alice_s_run(&repo).await;
    let key = StepKey { run_id, seq: 1 };
    let entry = EntryId::try_from("plan".to_string()).expect("entry");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.end_step(key, &Outcome::question(), "Which table?")
        .await
        .expect("end");
    repo.open_gate(key, "Which table?", &[Outcome::answer()])
        .await
        .expect("open");

    let refused = in_session(
        &router,
        Uuid::now_v7(),
        from_code(
            "POST",
            &format!("/runs/{run_id}/steps/1/answer"),
            serde_json::json!({ "answer": "answer", "note": "The new one" }),
        ),
    )
    .await;

    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    let refusal = body(refused).await;
    assert_eq!(refusal["code"], "reservedForAPerson");
    assert!(refusal["message"].is_string());
}

/// A Run refused at the Queue limit, one place held under a limit of 1,
/// says so with a code, to the person and to an agent starting it for
/// them alike.
#[tokio::test]
async fn a_run_refused_at_the_queue_limit_says_why() {
    let repo = FakeRepo::default();
    let router = records_router(&repo);
    let alice = MacroUserIdStr::try_from(ALICE.to_string()).expect("valid");
    let team_id = Uuid::now_v7();
    repo.teams.lock().expect("teams").insert(
        alice.to_string(),
        TeamMembership {
            team_id,
            admin: false,
        },
    );
    repo.team_rules.lock().expect("rules").insert(
        team_id,
        OrchestratorDefinition {
            queue_limit: Some(1),
            ..OrchestratorDefinition::DEFAULT
        },
    );
    let run_id = alice_s_run(&repo).await;
    let key = StepKey { run_id, seq: 1 };
    let entry = EntryId::try_from("plan".to_string()).expect("entry");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.open_gate(
        key,
        "Approve the plan?",
        &[Outcome::try_from("approve".to_string()).expect("valid")],
    )
    .await
    .expect("open");
    let start = serde_json::json!({ "flow": "reply", "request": "Say hello" });

    let person = router
        .clone()
        .oneshot(call("POST", "/runs", Caller::User("alice"), start.clone()))
        .await
        .expect("response");
    let agent = in_session(&router, Uuid::now_v7(), from_code("POST", "/runs", start)).await;

    for refused in [person, agent] {
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let refusal = body(refused).await;
        assert_eq!(refusal["code"], "queueFull");
        assert!(
            refusal["message"]
                .as_str()
                .is_some_and(|message| message.contains("queue limit")),
            "{refusal}"
        );
    }
}
