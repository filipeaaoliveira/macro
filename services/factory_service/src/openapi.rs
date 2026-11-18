//! Prints the Run API's OpenAPI document.

use factory_engine::inbound::automations::AutomationsApiDoc;
use factory_engine::inbound::evaluation::EvaluationApiDoc;
use factory_engine::inbound::http::ApiDoc;
use factory_engine::inbound::insights::InsightsApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), serde_json::Error> {
    let mut openapi = ApiDoc::openapi();
    openapi.merge(InsightsApiDoc::openapi());
    openapi.merge(AutomationsApiDoc::openapi());
    openapi.merge(EvaluationApiDoc::openapi());
    println!("{}", openapi.to_pretty_json()?);
    Ok(())
}
