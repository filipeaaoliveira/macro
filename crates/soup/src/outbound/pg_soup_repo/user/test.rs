use super::*;
use macro_user_id::user_id::MacroUserIdStr;
use model_entity::Entity;

#[test]
fn hydrates_only_the_viewer() -> anyhow::Result<()> {
    let entities: Vec<Entity<'static>> = vec![
        EntityType::User.with_entity_string("macro|user-2@example.com".to_string()),
        EntityType::User.with_entity_string("macro|user-1@example.com".to_string()),
        EntityType::Team.with_entity_string("macro|user-1@example.com".to_string()),
    ];
    let request = |user: &'static str| -> anyhow::Result<_> {
        Ok(AdvancedSortParams {
            entities: &entities,
            user_id: MacroUserIdStr::parse_from_str(user)?,
        })
    };

    let items = by_ids(request("macro|user-1@example.com")?);
    let [SoupItem::User(user)] = items.as_slice() else {
        panic!("expected only user-1, got {items:?}");
    };
    assert_eq!(user.id, "macro|user-1@example.com");

    // Someone the request does not name sees no one, not even the others.
    let outsider = by_ids(request("macro|outsider@example.com")?);
    assert!(outsider.is_empty(), "{outsider:?}");
    Ok(())
}
