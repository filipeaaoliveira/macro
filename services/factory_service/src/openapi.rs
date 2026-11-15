//! Prints the Run API's OpenAPI document.

use factory_engine::inbound::http::ApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), serde_json::Error> {
    println!("{}", ApiDoc::openapi().to_pretty_json()?);
    Ok(())
}
