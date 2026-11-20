use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The viewer, listed only by attention feeds: a notice about the viewer
/// alone (a factory Run started without a task waiting on its owner) is
/// filed under their User entity, which has no page of its own. Only the
/// viewer is ever listed, so no one sees another person's.
#[derive(Serialize, Clone, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SoupUser {
    /// The viewer's Macro user id, which their notices are filed under.
    pub id: String,
}

impl SoupUser {
    /// A uuid standing for the user where an item needs one, such as a
    /// Soup cursor: a user's id is not a uuid, and an attention feed pages
    /// by entity id instead.
    pub fn uuid(&self) -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_URL, self.id.as_bytes())
    }
}
