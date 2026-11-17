//! Making a follow-up task against a stand-in for document storage.

use document_storage_service_client::DocumentStorageServiceClient;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{DocumentTaskDirectory, may_add_to};
use crate::domain::ports::{FollowUpMade, FollowUpTask};
use crate::outbound::task_project::fixture::{new_task_property_values, task_properties};
use entity_access::domain::models::{
    AccessError, AccessLevel, BotAccessScope, BotId, CallChannelInfo, EntityAccessReceipt,
    EntityPermission, EntityType, RequiredPermission, TeamRole, UserTeamInfo,
};
use entity_access::domain::ports::EntityAccessService;
use macro_user_id::lowercased::Lowercase;
use macro_user_id::user_id::MacroUserId;

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

fn follow_up() -> FollowUpTask {
    FollowUpTask {
        name: "Follow-up: docs name the old flag".to_string(),
        markdown: "Found in review.".to_string(),
    }
}

/// The task document `id`, as document storage reads it.
fn basic(id: Uuid) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "documentId": id.to_string(),
        "documentName": "Follow-up",
        "owner": "macro|julia@example.com",
        "deletedAt": null
    }))
}

/// Made as document storage answers: `id`'s read answering `found`, in
/// order, and the make answering `made`, if it is asked.
async fn make(id: Uuid, found: &[bool], made: Option<ResponseTemplate>) -> (FollowUpMade, usize) {
    let server = MockServer::start().await;
    for (index, there) in found.iter().enumerate() {
        let answer = if *there {
            basic(id)
        } else {
            ResponseTemplate::new(404)
        };
        let reads = Mock::given(method("GET"))
            .and(path(format!("/internal/documents/{id}/basic")))
            .respond_with(answer)
            .up_to_n_times(1);
        reads
            .with_priority(u8::try_from(index + 1).expect("few"))
            .mount(&server)
            .await;
    }
    if let Some(made) = made {
        Mock::given(method("POST"))
            .and(path("/documents/create_task"))
            .respond_with(made)
            .mount(&server)
            .await;
    }
    let directory = DocumentTaskDirectory {
        access: (),
        documents: DocumentStorageServiceClient::new("key".into(), server.uri()),
    };

    let outcome = directory
        .make_follow_up(&julia(), None, &follow_up(), id, async |_| Ok(true))
        .await
        .expect("answered");
    let posts = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() == "POST")
        .count();
    (outcome, posts)
}

/// A task an earlier attempt made is found by its id, and not made again.
#[tokio::test]
async fn a_task_already_made_is_found_not_made_again() {
    let id = Uuid::now_v7();

    let (outcome, posts) = make(id, &[true], None).await;

    assert_eq!(outcome, FollowUpMade::Made);
    assert_eq!(posts, 0);
}

/// A refusal after which the task is there is the conflict of an attempt
/// that made it meanwhile; one after which it is not is a refusal.
#[tokio::test]
async fn a_conflict_is_made_when_the_task_is_there_and_refused_when_not() {
    let id = Uuid::now_v7();

    let (found, _) = make(id, &[false, true], Some(ResponseTemplate::new(409))).await;
    let (refused, _) = make(id, &[false, false], Some(ResponseTemplate::new(409))).await;

    assert_eq!(found, FollowUpMade::Made);
    assert_eq!(refused, FollowUpMade::Refused);
}

/// A task made under another id than the one asked for is told apart, so
/// the round stops rather than make one on every retry.
#[tokio::test]
async fn a_task_made_under_another_id_is_told_apart() {
    let id = Uuid::now_v7();
    let other = Uuid::now_v7();
    let answer =
        ResponseTemplate::new(200).set_body_json(json!({ "documentId": other.to_string() }));

    let (same, _) = make(
        id,
        &[false],
        Some(ResponseTemplate::new(200).set_body_json(json!({ "documentId": id.to_string() }))),
    )
    .await;
    let (elsewhere, _) = make(id, &[false], Some(answer)).await;

    assert_eq!(same, FollowUpMade::Made);
    assert_eq!(elsewhere, FollowUpMade::OtherId(other.to_string()));
}

/// A follow-up goes in the project its task's Project property names, as
/// a Main Hold's task does, though the task is in no folder of a project.
/// It is assigned to its owner and not started, as document storage makes
/// a task with no properties. One in a project the owner may no longer add
/// to (document storage would answer 401, as for a key it does not accept)
/// is made in no project, and says so.
#[tokio::test]
async fn a_follow_up_goes_in_the_project_its_task_s_project_property_names() {
    let id = Uuid::now_v7();
    let of = Uuid::now_v7();
    let project = Uuid::now_v7().to_string();

    for may in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/internal/documents/{id}/basic")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/internal/documents/{of}/basic")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "documentId": of.to_string(),
                "documentName": "Add dark mode",
                "owner": "macro|julia@example.com",
                "deletedAt": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/properties/entities/DOCUMENT/{of}")))
            .and(header(
                "x-document-storage-service-user-id",
                "macro|julia@example.com",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(task_properties(&of.to_string(), &project)),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/documents/create_task"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "documentId": id.to_string() })),
            )
            .mount(&server)
            .await;
        let directory = DocumentTaskDirectory {
            access: (),
            documents: DocumentStorageServiceClient::new("key".into(), server.uri()),
        };
        let asked = std::sync::Mutex::new(Vec::new());

        let outcome = directory
            .make_follow_up(&julia(), Some(of), &follow_up(), id, async |checked| {
                asked.lock().expect("asked").push(checked);
                Ok(may)
            })
            .await
            .expect("answered");

        assert_eq!(
            *asked.lock().expect("asked"),
            std::slice::from_ref(&project),
            "may add: {may}"
        );
        let posts: Vec<serde_json::Value> = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.method.as_str() == "POST")
            .map(|request| request.body_json().expect("json"))
            .collect();
        assert_eq!(outcome, FollowUpMade::Made);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0]["projectId"], json!(null));
        let markdown = posts[0]["markdown"].as_str().expect("markdown");
        if may {
            assert_eq!(
                posts[0]["propertyValues"],
                new_task_property_values("macro|julia@example.com", Some(&project))
            );
            assert_eq!(markdown, "Found in review.");
        } else {
            assert_eq!(
                posts[0]["propertyValues"],
                new_task_property_values("macro|julia@example.com", None)
            );
            assert!(markdown.starts_with("Found in review.\n\n"), "{markdown}");
            assert!(markdown.contains("in no project"), "{markdown}");
        }
    }
}

/// Entity access that gives one level on any entity, and keeps what it
/// was asked about.
#[derive(Clone, Default)]
struct OneLevel {
    level: Option<AccessLevel>,
    asked: std::sync::Arc<std::sync::Mutex<Vec<(String, EntityType)>>>,
}

impl EntityAccessService for OneLevel {
    async fn generate_entity_access_receipt<T: RequiredPermission>(
        &self,
        _: &MacroUserId<Lowercase<'_>>,
        _: Option<i64>,
        _: &str,
        _: EntityType,
    ) -> Result<EntityAccessReceipt<T>, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn generate_bot_entity_access_receipt<T: RequiredPermission>(
        &self,
        _: BotId,
        _: BotAccessScope,
        _: &str,
        _: EntityType,
    ) -> Result<EntityAccessReceipt<T>, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn get_access_level(
        &self,
        _: Option<&MacroUserId<Lowercase<'_>>>,
        entity_id: &str,
        entity_type: EntityType,
    ) -> Result<Option<AccessLevel>, AccessError> {
        self.asked
            .lock()
            .expect("asked")
            .push((entity_id.to_string(), entity_type));
        Ok(self.level)
    }
    async fn check_access(
        &self,
        _: Option<&MacroUserId<Lowercase<'_>>>,
        _: &str,
        _: EntityType,
        _: AccessLevel,
    ) -> Result<AccessLevel, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn check_public_access(
        &self,
        _: &str,
        _: EntityType,
        _: AccessLevel,
    ) -> Result<AccessLevel, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn get_entity_permission(
        &self,
        _: Option<&MacroUserId<Lowercase<'_>>>,
        _: &str,
        _: EntityType,
        _: Option<i64>,
    ) -> Result<EntityPermission, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn get_crm_entity_permission_with_team(
        &self,
        _: Option<&MacroUserId<Lowercase<'_>>>,
        _: &str,
        _: EntityType,
    ) -> Result<(EntityPermission, Uuid, TeamRole), AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn get_users_by_entity(
        &self,
        _: &str,
        _: EntityType,
    ) -> Result<Vec<MacroUserIdStr<'static>>, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn get_call_channel(&self, _: &Uuid) -> Result<Option<CallChannelInfo>, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn get_call_channel_by_channel_id(
        &self,
        _: &Uuid,
    ) -> Result<Option<CallChannelInfo>, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
    async fn get_user_team(
        &self,
        _: &MacroUserId<Lowercase<'_>>,
    ) -> Result<Option<UserTeamInfo>, AccessError> {
        unimplemented!("a follow-up only reads an access level")
    }
}

/// The owner may add a follow-up to an initiative they may edit or own,
/// and not to one they may only view, comment on, or not reach: entity
/// access is asked about the initiative, not a project folder.
#[tokio::test]
async fn a_follow_up_may_go_in_an_initiative_the_owner_may_edit() {
    let initiative = Uuid::now_v7().to_string();
    for (level, may) in [
        (Some(AccessLevel::Owner), true),
        (Some(AccessLevel::Edit), true),
        (Some(AccessLevel::Comment), false),
        (Some(AccessLevel::View), false),
        (None, false),
    ] {
        let access = OneLevel {
            level,
            ..OneLevel::default()
        };

        let allowed = may_add_to(&access, &julia(), initiative.clone())
            .await
            .expect("checked");

        assert_eq!(allowed, may, "{level:?}");
        assert_eq!(
            *access.asked.lock().expect("asked"),
            [(initiative.clone(), EntityType::Initiative)],
            "{level:?}"
        );
    }
}
