//! Prints the Run API's OpenAPI document.

use factory_engine::inbound::http::ApiDoc;
use factory_engine::inbound::insights::InsightsApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), serde_json::Error> {
    let mut openapi = ApiDoc::openapi();
    openapi.merge(InsightsApiDoc::openapi());
    println!("{}", openapi.to_pretty_json()?);
    Ok(())
}
