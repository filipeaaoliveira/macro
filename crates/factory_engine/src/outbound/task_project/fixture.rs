//! A task's properties as document storage answers for them.

use serde_json::json;

/// The id of the Project property's definition.
pub(crate) const PROJECT: &str = "00000001-0000-0000-0000-000000000014";

/// The properties of the task `task`, as document storage answers
/// `GET /properties/entities/DOCUMENT/{task}`: its Status, and its Project
/// naming `initiative`.
pub(crate) fn task_properties(task: &str, initiative: &str) -> serde_json::Value {
    json!({
        "entity_id": task,
        "properties": [
            {
                "property": {
                    "id": "01a1eb0a-9a05-7aec-8466-2c552bb777e7",
                    "entity_id": task,
                    "entity_type": "TASK",
                    "property_definition_id": "00000001-0000-0000-0000-000000000002",
                    "created_at": "2026-11-17T19:45:02.982493Z",
                    "updated_at": "2026-11-17T19:45:02.982493Z"
                },
                "definition": {
                    "id": "00000001-0000-0000-0000-000000000002",
                    "owner": { "scope": "system" },
                    "display_name": "Status",
                    "data_type": "SELECT_STRING",
                    "is_multi_select": false,
                    "specific_entity_type": null,
                    "created_at": "2026-11-15T17:36:34.117779Z",
                    "updated_at": "2026-11-15T17:36:34.117779Z",
                    "is_system": true,
                    "is_metadata": false
                },
                "value": {
                    "type": "SelectOption",
                    "value": ["00000001-0000-0000-0002-000000000001"]
                },
                "options": null
            },
            {
                "property": {
                    "id": "01a1eb0a-9a45-712f-be66-f42e2d90e13c",
                    "entity_id": task,
                    "entity_type": "TASK",
                    "property_definition_id": PROJECT,
                    "created_at": "2026-11-17T19:45:03.040992Z",
                    "updated_at": "2026-11-17T19:45:03.040992Z"
                },
                "definition": {
                    "id": PROJECT,
                    "owner": { "scope": "system" },
                    "display_name": "Project",
                    "data_type": "ENTITY",
                    "is_multi_select": false,
                    "specific_entity_type": "INITIATIVE",
                    "created_at": "2026-11-15T17:36:36.980465Z",
                    "updated_at": "2026-11-15T17:36:36.980465Z",
                    "is_system": true,
                    "is_metadata": false
                },
                "value": {
                    "type": "EntityReference",
                    "value": [{ "entity_id": initiative, "entity_type": "INITIATIVE" }]
                },
                "options": null
            }
        ]
    })
}
