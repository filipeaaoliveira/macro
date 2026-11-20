//! The names GitHub gives accounts and repositories. A request names a
//! repository by these parts, and they are written into GitHub API paths
//! as they come, so anything else (a `/`, `..`, `%`, `?` or `#`) could
//! reach another repository or pull request than the one named.

#[cfg(test)]
mod test;

/// The longest account name GitHub allows.
const OWNER_MAX: usize = 39;

/// The longest repository name GitHub allows.
const REPOSITORY_MAX: usize = 100;

/// Whether `owner` is a GitHub account name: letters, digits, `-`, and `_`,
/// which Enterprise Managed Users' accounts end with (`handle_shortcode`).
pub fn is_github_owner(owner: &str) -> bool {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
    !owner.is_empty() && owner.len() <= OWNER_MAX && owner.chars().all(allowed)
}

/// Whether `name` is a GitHub repository name: letters, digits, `.`, `_`
/// and `-`, other than `.` and `..`. A name ending `.git` is refused too:
/// GitHub reads it as the repository without that ending, so it would not
/// name the pull request it seems to.
pub fn is_github_repository_name(name: &str) -> bool {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    let reserved = name == "." || name == "..";
    let git_ending = name.to_ascii_lowercase().ends_with(".git");

    !name.is_empty()
        && name.len() <= REPOSITORY_MAX
        && name.chars().all(allowed)
        && !reserved
        && !git_ending
}
