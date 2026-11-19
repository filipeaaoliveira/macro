//! The viewer for attention feeds. A User has no list of its own in Soup: it
//! is a candidate only through the notices about the viewer alone filed under
//! it (a factory Run started without a task waiting on its owner), and only
//! for that viewer, so no one is listed another person's notices.

#[cfg(test)]
mod test;

use model_entity::EntityType;
use models_soup::{item::SoupItem, user::SoupUser};

use crate::domain::models::AdvancedSortParams;
use crate::outbound::pg_soup_repo::notified::ID_SQL;

/// The attention gate: the candidate names the viewer `$1` themself.
pub(super) fn user_gate() -> String {
    format!("{ID_SQL} = $1")
}

/// The viewer, when the request names them; any other user is no item.
/// It reads nothing: the viewer is all a User item says.
pub(super) fn by_ids(req: AdvancedSortParams<'_>) -> Vec<SoupItem<()>> {
    let viewer = req.user_id.as_ref();
    req.entities
        .iter()
        .find(|entity| entity.entity_type == EntityType::User && entity.entity_id == viewer)
        .map(|_| {
            SoupItem::User(SoupUser {
                id: viewer.to_string(),
            })
        })
        .into_iter()
        .collect()
}
