//! Which workspace package holds each file, from `cargo metadata --no-deps`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[cfg(test)]
mod test;

/// The part of `cargo metadata --format-version 1 --no-deps` the check reads.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    packages: Vec<Package>,
    workspace_root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
}

impl Metadata {
    /// Groups `files`, paths from the workspace root, by the package whose
    /// folder holds each most closely. A file outside every package is left
    /// out: it is not in the workspace, so no package's tests cover it.
    pub fn packages_of(&self, files: &[String]) -> BTreeMap<String, Vec<String>> {
        let mut packages: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for file in files {
            if let Some(name) = self.package_of(Path::new(file)) {
                packages
                    .entry(name.to_owned())
                    .or_default()
                    .push(file.clone());
            }
        }
        packages
    }

    /// The `files` no package holds, such as those of another workspace in
    /// the repository: their tests never run here, so they are not scored.
    pub fn outside(&self, files: &[String]) -> Vec<String> {
        files
            .iter()
            .filter(|file| self.package_of(Path::new(file)).is_none())
            .cloned()
            .collect()
    }

    fn package_of(&self, file: &Path) -> Option<&str> {
        self.packages
            .iter()
            .filter_map(|package| {
                let folder = package.manifest_path.parent()?;
                let folder = folder.strip_prefix(&self.workspace_root).ok()?;
                file.starts_with(folder)
                    .then_some((folder.components().count(), package.name.as_str()))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, name)| name)
    }
}
