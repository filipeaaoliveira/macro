//! Worktrees for sessions that name the branch they work on.
//!
//! A macrod that runs no Herdr serves every session in its one workspace,
//! the operator's checkout. A session requested with a `work_branch` (a
//! factory Run's `factory/<run id>`) works instead in a git worktree of
//! that checkout made for the branch, `<checkout>/.macrod-worktrees/<key>`,
//! which every session naming the same branch shares: a Run's later Steps
//! find its earlier Steps' work there. Sessions on other branches, and the
//! checkout itself, never share its files or its `HEAD`, so two Runs at
//! once cannot commit on each other's branch.
//!
//! The worktree starts detached, at the branch as origin has it when origin
//! has it, else at origin's default branch: the session checks the branch
//! out, or creates it from the base it is told of. Making it never touches
//! the checkout's branch, index or files; git is told to leave the
//! directory out of the checkout's status (`info/exclude`).
//!
//! Each prepare removes worktrees no session asked for in
//! [`STALE_AFTER`], and those whose branch origin had and no longer has
//! (a merged Run's, deleted after its merge) once no session asked for them
//! in [`GONE_AFTER`].

#[cfg(test)]
mod test;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::outbound::git::git;

/// The directory, in the checkout, that holds the worktrees.
pub(crate) const DIRECTORY: &str = ".macrod-worktrees";

/// How long a worktree no session asked for is kept.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How long a worktree whose branch origin deleted is kept after a session
/// last asked for it.
pub(crate) const GONE_AFTER: Duration = Duration::from_secs(60 * 60);

/// The worktrees of one checkout, made one at a time.
pub(crate) struct BranchWorktrees {
    checkout: PathBuf,
    preparing: tokio::sync::Mutex<()>,
}

impl BranchWorktrees {
    /// The worktrees of `checkout`, the workspace sessions share.
    pub(crate) fn new(checkout: PathBuf) -> Self {
        Self {
            checkout,
            preparing: tokio::sync::Mutex::new(()),
        }
    }

    /// The worktree sessions on `branch` work in, made when there is none
    /// yet. None when the checkout is no git checkout: there is nothing to
    /// make a worktree of.
    pub(crate) async fn prepare(&self, branch: &str) -> rootcause::Result<Option<PathBuf>> {
        let key = key(branch)?;
        let _guard = self.preparing.lock().await;
        let Ok(top) = git(&self.checkout, &["rev-parse", "--show-toplevel"]).await else {
            return Ok(None);
        };
        let top = PathBuf::from(top);
        let directory = top.join(DIRECTORY);
        std::fs::create_dir_all(&directory)?;
        exclude(&top).await?;
        remove_stale(&top, &directory, &key).await;

        let path = directory.join(&key);
        if path.exists() {
            if is_worktree_of(&path, &top).await {
                touch(&directory, &key)?;
                return Ok(Some(path));
            }
            // An earlier prepare stopped half way.
            remove(&top, &directory, &key).await;
        }

        let tracking = format!("refs/remotes/origin/{branch}");
        // A branch origin does not have yet fails to fetch, and starts
        // from origin's default branch instead.
        let _ = git(
            &top,
            &[
                "fetch",
                "--no-tags",
                "origin",
                &format!("+refs/heads/{branch}:{tracking}"),
            ],
        )
        .await;
        let mut start = None;
        for candidate in [
            tracking.as_str(),
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
            "HEAD",
        ] {
            let commit = format!("{candidate}^{{commit}}");
            if let Ok(found) = git(&top, &["rev-parse", "--verify", "--quiet", &commit]).await {
                start = Some(found);
                break;
            }
        }
        let start =
            start.ok_or_else(|| rootcause::report!("the checkout has no commit to start from"))?;
        git(
            &top,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                &path.to_string_lossy(),
                &start,
            ],
        )
        .await?;
        touch(&directory, &key)?;
        Ok(Some(path))
    }
}

/// `branch` as one directory name: a branch macrod accepts (letters,
/// digits, `-`, `_`, `.` and `/` between parts that are not `.`, `..` or
/// empty, and do not start with `.` or `-`) with each `/` written `+`,
/// which no accepted branch holds.
pub(crate) fn key(branch: &str) -> rootcause::Result<String> {
    let valid = !branch.is_empty()
        && branch.len() <= 200
        && !branch.ends_with(".lock")
        && branch.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.starts_with('-')
                && !part.ends_with('.')
                && !part.contains("..")
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        });
    if !valid {
        rootcause::bail!("the session's work branch is not a branch macrod accepts: {branch:?}");
    }
    Ok(branch.replace('/', "+"))
}

/// The branch a worktree's directory name `key` stands for.
fn branch_of(key: &str) -> String {
    key.replace('+', "/")
}

/// Leave [`DIRECTORY`] out of the checkout's status.
async fn exclude(top: &Path) -> rootcause::Result<()> {
    let file = git(top, &["rev-parse", "--git-path", "info/exclude"]).await?;
    let file = top.join(file);
    let line = format!("/{DIRECTORY}/");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.lines().any(|existing| existing.trim() == line) {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(&file, format!("{text}{separator}{line}\n"))?;
    Ok(())
}

/// Whether `path` is a worktree of the repository checked out at `top`.
async fn is_worktree_of(path: &Path, top: &Path) -> bool {
    let (Ok(ours), Ok(theirs)) = (
        git(
            path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await,
        git(
            top,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await,
    ) else {
        return false;
    };
    let toplevel = git(path, &["rev-parse", "--show-toplevel"]).await;
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    same(Path::new(&ours), Path::new(&theirs))
        && toplevel.is_ok_and(|toplevel| same(Path::new(&toplevel), path))
}

/// The file whose time says when a session last asked for `key`'s worktree.
fn marker(directory: &Path, key: &str) -> PathBuf {
    directory.join(format!("{key}.used"))
}

/// Say a session asked for `key`'s worktree now.
fn touch(directory: &Path, key: &str) -> std::io::Result<()> {
    std::fs::write(marker(directory, key), b"")
}

/// When a session last asked for `key`'s worktree: its marker's time, else
/// the worktree's own.
fn last_used(directory: &Path, key: &str) -> Option<SystemTime> {
    std::fs::metadata(marker(directory, key))
        .or_else(|_| std::fs::metadata(directory.join(key)))
        .and_then(|metadata| metadata.modified())
        .ok()
}

/// Remove `key`'s worktree, and its marker.
async fn remove(top: &Path, directory: &Path, key: &str) {
    let path = directory.join(key);
    if git(
        top,
        &["worktree", "remove", "--force", &path.to_string_lossy()],
    )
    .await
    .is_err()
    {
        let _ = std::fs::remove_dir_all(&path);
    }
    let _ = git(top, &["worktree", "prune"]).await;
    let _ = std::fs::remove_file(marker(directory, key));
}

/// Remove the worktrees no session asked for in [`STALE_AFTER`], and those
/// whose branch origin deleted that no session asked for in
/// [`GONE_AFTER`]; never `keep`'s. Best effort: what fails stays.
async fn remove_stale(top: &Path, directory: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    let mut idle = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let key = entry.file_name().to_string_lossy().into_owned();
        if key == keep || self::key(&branch_of(&key)).ok().as_deref() != Some(key.as_str()) {
            continue;
        }
        let Some(age) = last_used(directory, &key).and_then(|used| now.duration_since(used).ok())
        else {
            continue;
        };
        if age >= STALE_AFTER {
            tracing::info!(branch = %branch_of(&key), "removing a worktree no session used");
            remove(top, directory, &key).await;
        } else if age >= GONE_AFTER {
            idle.push(key);
        }
    }

    // Only a branch origin had, as this clone saw it, and lists no longer
    // was deleted there: one not pushed yet is still being worked on.
    let mut tracked = Vec::new();
    for key in idle {
        let tracking = format!("refs/remotes/origin/{}", branch_of(&key));
        if git(top, &["rev-parse", "--verify", "--quiet", &tracking])
            .await
            .is_ok()
        {
            tracked.push(key);
        }
    }
    if tracked.is_empty() {
        return;
    }
    let Ok(listed) = git(top, &["ls-remote", "--heads", "origin"]).await else {
        return;
    };
    let heads: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|name| name.strip_prefix("refs/heads/"))
        .collect();
    for key in tracked {
        let branch = branch_of(&key);
        if !heads.contains(&branch.as_str()) {
            tracing::info!(%branch, "removing the worktree of a branch origin deleted");
            remove(top, directory, &key).await;
            let tracking = format!("refs/remotes/origin/{branch}");
            let _ = git(top, &["update-ref", "-d", &tracking]).await;
        }
    }
}
