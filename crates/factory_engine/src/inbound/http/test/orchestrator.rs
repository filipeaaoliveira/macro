//! The Orchestrator through the Run API: an agent session in Macro chat
//! answers Gates for the person from its code, with their own access, and
//! is refused what the factory keeps for them in person; and a Run refused
//! above the Queue limit says why.

use agent_correlation::context::{Attribution, scope_attribution};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use factory_records::domain::models::{EntryId, Outcome, StepKey, TeamMembership};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::OrchestratorDefinition;
use macro_authorization::CODE_EXECUTION_HEADER;
use macro_user_id::user_id::MacroUserIdStr;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{ALICE, Caller, alice_s_run, body, call, records_router, router};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::{Answerer, FactoryError};

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

/// A Run refused above the Queue limit says so with a code, to the person
/// and to an agent starting it for them alike.
#[tokio::test]
async fn a_run_refused_above_the_queue_limit_says_why() {
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
            queue_limit: Some(0),
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
