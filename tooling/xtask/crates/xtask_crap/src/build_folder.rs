//! Which folder of the shared coverage build a repository gets.

#[cfg(test)]
mod test;

/// A folder name for the repository at `remote` (its `origin` URL, or its
/// path without one), the same for its SSH and HTTPS URLs. Credentials in the
/// URL are left out, and every character but letters, digits, `.`, `_` and
/// `-` becomes `-`.
pub fn repository_folder(remote: &str) -> String {
    let remote = remote.trim();
    let without_scheme = remote.split_once("://").map_or(remote, |(_, rest)| rest);
    let without_user = without_scheme
        .rsplit_once('@')
        .map_or(without_scheme, |(_, rest)| rest);
    let name = without_user.trim_end_matches('/').trim_end_matches(".git");

    let folder: String = name
        .chars()
        .map(|character| {
            let kept = character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-');
            if kept { character } else { '-' }
        })
        .collect();
    folder.trim_matches('-').to_owned()
}
