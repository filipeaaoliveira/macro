//! Checks a Factory Definition in a folder, as a Run would read it from its
//! repository: `factory_definition_check [folder]`, the current folder when
//! none is given. It prints what the Definition holds, or the first problem
//! it finds with the file it is in, and then exits 1.

use std::process::ExitCode;

use factory_engine::domain::built_in_steps::built_in_step_use;
use factory_engine::domain::definition::{DefinitionReadError, read_definition};
use factory_engine::outbound::local_definition_files::LocalDefinitionFiles;

#[tokio::main]
async fn main() -> ExitCode {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    match read_definition(&LocalDefinitionFiles::new(&root), built_in_step_use).await {
        Ok(definition) => {
            let flows: Vec<&str> = definition.flows.keys().map(String::as_str).collect();
            println!(
                "{root}: the definition reads: flows {}, {} scorers",
                flows.join(", "),
                definition.scorers.len()
            );
            ExitCode::SUCCESS
        }
        Err(DefinitionReadError::Invalid(error)) => {
            eprintln!("{root}: {error}");
            ExitCode::FAILURE
        }
        Err(DefinitionReadError::Files(error)) => {
            eprintln!("{root}: a file could not be read: {error}");
            ExitCode::FAILURE
        }
    }
}
