//! Service layer composite entity property with definition model.

use serde::{Deserialize, Serialize};

use super::entity_property::EntityProperty;
use super::property_definition::PropertyDefinition;
use super::property_option::PropertyOption;
use super::property_value::PropertyValue;

/// Entity property with its definition, value, and options (service representation).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EntityPropertyWithDefinition {
    pub property: EntityProperty,
    pub definition: PropertyDefinition,
    pub value: Option<PropertyValue>,
    pub options: Option<Vec<PropertyOption>>,
}

// ===== Conversions =====

impl From<EntityPropertyWithDefinition> for crate::api::EntityPropertyWithDefinitionResponse {
    fn from(svc: EntityPropertyWithDefinition) -> Self {
        Self {
            property: svc.property.into(),
            definition: svc.definition.into(),
            value: svc.value.map(Into::into),
            options: svc
                .options
                .map(|opts| opts.into_iter().map(Into::into).collect()),
        }
    }
}

// The properties API answers `GET /properties/entities/{type}/{id}` with it,
// and its clients read it here, so both sides share one shape. Its doc line
// is its OpenAPI description.
/// Response for document/entity properties endpoint.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EntityPropertiesResponse {
    pub entity_id: String,
    pub properties: Vec<EntityPropertyWithDefinition>,
}
