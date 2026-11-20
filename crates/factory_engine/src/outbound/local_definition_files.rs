//! A Factory Definition's files in a folder of this machine: a checkout of
//! its repository, for the validator to read.

use std::path::PathBuf;

use crate::domain::definition::DefinitionFiles;

/// The files of a Definition under one folder.
#[derive(Debug, Clone)]
pub struct LocalDefinitionFiles {
    root: PathBuf,
}

impl LocalDefinitionFiles {
    /// The Definition whose `factory.json` is in `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl DefinitionFiles for LocalDefinitionFiles {
    type Err = std::io::Error;

    async fn read(&self, path: &str) -> Result<Option<String>, Self::Err> {
        match tokio::fs::read_to_string(self.root.join(path)).await {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}
