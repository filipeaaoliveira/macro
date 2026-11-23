//! A change to a team's Factory Definition, proposed from the factory's
//! settings: the whole new text of some of its files. It is checked as the
//! Definition would read with it, at the commit its default branch is at,
//! before it goes to the repository as a pull request a person merges.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use super::{
    DefinitionError, DefinitionFiles, DefinitionReadError, FactoryDefinition, MAX_FLOWS,
    read_definition,
};
use crate::domain::flow::{StepName, StepUse};

/// The most files one change writes: every file a Definition reads.
pub const MAX_CHANGED_FILES: usize = MAX_FLOWS + 4;

/// The most bytes a change's title holds.
pub const MAX_CHANGE_TITLE_BYTES: usize = 256;

/// The most bytes a change's description holds.
pub const MAX_CHANGE_DESCRIPTION_BYTES: usize = 16 * 1024;

/// A change to a Factory Definition's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionChange {
    /// What it changes, in a line: its pull request's title and its
    /// commit's message.
    pub title: String,
    /// Why, for the person who reviews it.
    pub description: String,
    /// The files it writes, by their path from the repository's root, each
    /// with its whole new text.
    pub files: BTreeMap<String, String>,
}

/// Why a change to a Factory Definition is refused.
#[derive(Debug, thiserror::Error)]
pub enum DefinitionChangeError {
    /// It writes no file, or more than a Definition has.
    #[error("a change must write from 1 to {MAX_CHANGED_FILES} files")]
    FileCount,
    /// Its title is blank or too long.
    #[error(
        "a change's title must not be blank, and must be at most {MAX_CHANGE_TITLE_BYTES} bytes"
    )]
    Title,
    /// Its description is too long.
    #[error("a change's description must be at most {MAX_CHANGE_DESCRIPTION_BYTES} bytes")]
    Description,
    /// The Definition, with the change, does not read.
    #[error("the definition with this change does not read: {0}")]
    Invalid(DefinitionError),
    /// It writes a file the Definition, with the change, does not read: no
    /// check would see what it holds.
    #[error(
        "{0} is not a file the definition reads: a flow's file is read once factory.json names the flow"
    )]
    NotRead(String),
    /// Every file it writes already holds that text.
    #[error("the change leaves every file as it is")]
    Unchanged,
}

/// Why a change to a Factory Definition could not be checked.
#[derive(Debug, thiserror::Error)]
pub enum DefinitionChangeReadError<E> {
    /// A file of the Definition could not be read.
    #[error("the definition's files could not be read")]
    Files(#[source] E),
    /// The change is refused.
    #[error(transparent)]
    Refused(#[from] DefinitionChangeError),
}

impl DefinitionChange {
    /// A change titled `title`, described by `description`, that writes
    /// `files`.
    pub fn new(
        title: String,
        description: Option<String>,
        files: BTreeMap<String, String>,
    ) -> Result<Self, DefinitionChangeError> {
        if files.is_empty() || files.len() > MAX_CHANGED_FILES {
            return Err(DefinitionChangeError::FileCount);
        }
        let title = title.trim().to_string();
        if title.is_empty() || title.len() > MAX_CHANGE_TITLE_BYTES {
            return Err(DefinitionChangeError::Title);
        }
        let description = description.unwrap_or_default();
        if description.len() > MAX_CHANGE_DESCRIPTION_BYTES {
            return Err(DefinitionChangeError::Description);
        }
        Ok(Self {
            title,
            description,
            files,
        })
    }
}

/// A Definition's files with a change's written over them, keeping the
/// paths the Definition read.
struct ChangedFiles<'a, F> {
    base: &'a F,
    changed: &'a BTreeMap<String, String>,
    read: Mutex<BTreeSet<String>>,
}

impl<F: DefinitionFiles> DefinitionFiles for ChangedFiles<'_, F> {
    type Err = F::Err;

    async fn read(&self, path: &str) -> Result<Option<String>, Self::Err> {
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_string());
        match self.changed.get(path) {
            Some(text) => Ok(Some(text.clone())),
            None => self.base.read(path).await,
        }
    }
}

/// The Factory Definition `base` holds with `change` written over it, read
/// and checked as any Definition is (`serves` says which Steps the factory
/// runs). Each file the change writes must be one the Definition then reads,
/// so the checks see all it writes, and at least one must differ from
/// what `base` holds.
pub async fn read_changed_definition<F: DefinitionFiles>(
    base: &F,
    change: &DefinitionChange,
    serves: impl Fn(&StepName) -> Option<StepUse>,
) -> Result<FactoryDefinition, DefinitionChangeReadError<F::Err>> {
    let files = ChangedFiles {
        base,
        changed: &change.files,
        read: Mutex::new(BTreeSet::new()),
    };
    let definition = read_definition(&files, serves)
        .await
        .map_err(|error| match error {
            DefinitionReadError::Files(error) => DefinitionChangeReadError::Files(error),
            DefinitionReadError::Invalid(error) => DefinitionChangeError::Invalid(error).into(),
        })?;

    let read = files
        .read
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(unread) = change.files.keys().find(|path| !read.contains(*path)) {
        return Err(DefinitionChangeError::NotRead(unread.clone()).into());
    }

    for (path, text) in &change.files {
        let before = base
            .read(path)
            .await
            .map_err(DefinitionChangeReadError::Files)?;
        if before.as_ref() != Some(text) {
            return Ok(definition);
        }
    }
    Err(DefinitionChangeError::Unchanged.into())
}
