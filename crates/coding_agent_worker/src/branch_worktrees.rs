//! Worktrees for sessions that name the branch they work on.
//!
//! A macrod that runs no Herdr serves every session in its one workspace,
//! the operator's checkout. A session requested with a `work_branch` (a
//! factory Run's `factory/<run id>`) works instead in a git worktree of
//! that checkout made for the branch, `<checkout>.macrod-worktrees/<key>`,
//! beside the checkout rather than in it, so no tool reads the checkout's
//! own configuration or dependencies from it. Every session naming the same
//! branch shares it: a Run's later Steps find its earlier Steps' work
//! there. Sessions on other branches, and the checkout itself, never share
//! its files or its `HEAD`, so two Runs at once cannot commit on each
//! other's branch.
//!
//! The worktree starts detached, at the branch as origin has it when origin
//! has it, else at origin's default branch: the session checks the branch
//! out, or creates it from the base it is told of. Making it never touches
//! the checkout's branch, index or files.
//!
//! Each branch is prepared under a lock of its own, and only adding or
//! removing a worktree waits on the others. At most every
//! [`CLEAN_EVERY`], a prepare starts a sweep in the background that removes
//! the worktrees no session used in [`STALE_AFTER`], and those no session
//! used in [`GONE_AFTER`] whose branch origin no longer lists and that hold
//! nothing origin lacks (no change left uncommitted, no commit on no remote
//! branch). A session uses a worktree when it opens there, and when a turn
//! of it starts or ends ([`ActiveTurns::workplace`]); a worktree a session
//! has a turn running in is never removed. Removing one deletes the
//! branch's local ref with it.

#[cfg(test)]
mod test;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use crate::outbound::git::git;
use crate::turns::ActiveTurns;

/// What is appended to the checkout's directory name to name the directory
/// beside it that holds the worktrees.
pub(crate) const SUFFIX: &str = ".macrod-worktrees";

/// How long a worktree no session used is kept.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// How long a worktree whose branch origin no longer lists is kept after a
/// session last used it.
pub(crate) const GONE_AFTER: Duration = Duration::from_secs(60 * 60);

/// How often a prepare starts a sweep at most.
pub(crate) const CLEAN_EVERY: Duration = Duration::from_secs(5 * 60);

/// The worktrees of one checkout.
pub(crate) struct BranchWorktrees {
    checkout: PathBuf,
    turns: ActiveTurns,
    /// One lock per branch key, held while it is prepared or removed.
    keys: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Held while git adds, removes or prunes worktrees.
    admin: tokio::sync::Mutex<()>,
    /// When the last sweep started.
    swept: Mutex<Option<Instant>>,
}

impl BranchWorktrees {
    /// The worktrees of `checkout`, the workspace sessions share, whose
    /// sessions' turns `turns` sees.
    pub(crate) fn new(checkout: PathBuf, turns: ActiveTurns) -> Self {
        Self {
            checkout,
            turns,
            keys: Mutex::default(),
            admin: tokio::sync::Mutex::new(()),
            swept: Mutex::default(),
        }
    }

    /// The worktree sessions on `branch` work in, made when there is none
    /// yet; then a sweep in the background, when one is due. None when the
    /// checkout is no git checkout: there is nothing to make a worktree of.
    pub(crate) async fn prepare(
        self: &Arc<Self>,
        branch: &str,
    ) -> rootcause::Result<Option<PathBuf>> {
        let key = key(branch)?;
        let Some(top) = self.top().await else {
            return Ok(None);
        };
        let directory = directory(&top);
        let path = {
            let lock = self.key_lock(&key);
            let _guard = lock.lock().await;
            self.prepare_locked(&top, &directory, branch, &key).await?
        };
        if self.sweep_due() {
            let worktrees = Arc::clone(self);
            tokio::spawn(async move { worktrees.sweep(&key).await });
        }
        Ok(Some(path))
    }

    /// The checkout's top directory; none when it is no git checkout.
    async fn top(&self) -> Option<PathBuf> {
        git(&self.checkout, &["rev-parse", "--show-toplevel"])
            .await
            .ok()
            .map(PathBuf::from)
    }

    fn key_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.keys
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(key.to_owned())
                .or_default(),
        )
    }

    async fn prepare_locked(
        &self,
        top: &Path,
        directory: &Path,
        branch: &str,
        key: &str,
    ) -> rootcause::Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(key);
        if path.exists() {
            if is_worktree_of(&path, top).await {
                touch(directory, key, SystemTime::now())?;
                return Ok(path);
            }
            // An earlier prepare stopped half way.
            self.remove(top, directory, key).await;
        }

        let tracking = format!("refs/remotes/origin/{branch}");
        let fetched = git(
            top,
            &[
                "fetch",
                "--no-tags",
                "origin",
                &format!("+refs/heads/{branch}:{tracking}"),
            ],
        )
        .await;
        // A branch origin does not have yet starts from origin's default
        // branch instead.
        if let Err(error) = fetched
            && !error.to_string().contains("couldn't find remote ref")
        {
            tracing::warn!(%branch, %error, "the work branch could not be fetched");
        }
        let mut start = None;
        for candidate in [
            tracking.as_str(),
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
            "HEAD",
        ] {
            let commit = format!("{candidate}^{{commit}}");
            if let Ok(found) = git(top, &["rev-parse", "--verify", "--quiet", &commit]).await {
                start = Some(found);
                break;
            }
        }
        let start =
            start.ok_or_else(|| rootcause::report!("the checkout has no commit to start from"))?;
        {
            let _admin = self.admin.lock().await;
            git(
                top,
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
        }
        touch(directory, key, SystemTime::now())?;
        Ok(path)
    }

    /// Whether a sweep is due, marking one started when it is.
    fn sweep_due(&self) -> bool {
        let mut swept = self.swept.lock().unwrap_or_else(PoisonError::into_inner);
        if swept.is_some_and(|at| at.elapsed() < CLEAN_EVERY) {
            return false;
        }
        *swept = Some(Instant::now());
        true
    }

    /// Remove what [`Self::sweep`] says, now, sparing `keep`'s worktree.
    pub(crate) async fn sweep(&self, keep: &str) {
        let Some(top) = self.top().await else {
            return;
        };
        let directory = directory(&top);
        let Ok(entries) = std::fs::read_dir(&directory) else {
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
            match self.idle_for(&directory, &key, now) {
                Some(age) if age >= STALE_AFTER => {
                    self.remove_if_idle(&top, &directory, &key, STALE_AFTER)
                        .await;
                }
                Some(age) if age >= GONE_AFTER => idle.push(key),
                _ => {}
            }
        }
        if idle.is_empty() {
            return;
        }

        let Ok(listed) = git(&top, &["ls-remote", "--heads", "origin"]).await else {
            return;
        };
        let heads: Vec<&str> = listed
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1))
            .filter_map(|name| name.strip_prefix("refs/heads/"))
            .collect();
        for key in idle {
            let branch = branch_of(&key);
            if heads.contains(&branch.as_str()) {
                continue;
            }
            let path = directory.join(&key);
            if !holds_nothing_origin_lacks(&path).await {
                continue;
            }
            self.remove_if_idle(&top, &directory, &key, GONE_AFTER)
                .await;
        }
    }

    /// How long no session used `key`'s worktree: since its marker's time,
    /// or since a session last opened there or a turn there started or
    /// ended, whichever is later; that later time is kept in the marker.
    /// None while a session has a turn running there, or when no time is
    /// known.
    fn idle_for(&self, directory: &Path, key: &str, now: SystemTime) -> Option<Duration> {
        let path = directory.join(key);
        let (running, active) = self.turns.workplace(&path.to_string_lossy());
        if running {
            return None;
        }
        let marked = last_used(directory, key);
        let used = match (marked, active) {
            (Some(marked), Some(active)) if active > marked => {
                let _ = touch(directory, key, active);
                active
            }
            (Some(marked), _) => marked,
            (None, Some(active)) => active,
            // A worktree an earlier macrod made without its marker.
            (None, None) => std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok()?,
        };
        now.duration_since(used).ok()
    }

    /// Remove `key`'s worktree when no session used it in `idle`, under its
    /// lock; one being prepared is left alone.
    async fn remove_if_idle(&self, top: &Path, directory: &Path, key: &str, idle: Duration) {
        let lock = self.key_lock(key);
        let Ok(_guard) = lock.try_lock() else {
            return;
        };
        if self
            .idle_for(directory, key, SystemTime::now())
            .is_none_or(|age| age < idle)
        {
            return;
        }
        tracing::info!(branch = %branch_of(key), "removing a work branch's idle worktree");
        self.remove(top, directory, key).await;
    }

    /// Remove `key`'s worktree, its marker and its branch's local ref.
    async fn remove(&self, top: &Path, directory: &Path, key: &str) {
        let path = directory.join(key);
        let _admin = self.admin.lock().await;
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
        let _ = git(top, &["branch", "-D", &branch_of(key)]).await;
        let _ = std::fs::remove_file(marker(directory, key));
    }
}

/// The directory beside the checkout at `top` that holds its worktrees.
pub(crate) fn directory(top: &Path) -> PathBuf {
    let name = top
        .file_name()
        .map_or_else(|| "checkout".into(), |name| name.to_string_lossy());
    top.with_file_name(format!("{name}{SUFFIX}"))
}

/// `branch` as one directory name: a branch macrod accepts (lower-case
/// letters, digits, `-`, `_`, `.` and `/` between parts that are not `.`,
/// `..` or empty, and do not start with `.` or `-`; no `.lock` or `.used`
/// at its end) with each `/` written `+`, which no accepted branch holds.
pub(crate) fn key(branch: &str) -> rootcause::Result<String> {
    let valid = !branch.is_empty()
        && branch.len() <= 200
        && !branch.ends_with(".lock")
        && !branch.ends_with(".used")
        && branch.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.starts_with('-')
                && !part.ends_with('.')
                && !part.contains("..")
                && part.chars().all(|ch| {
                    ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_' | '.')
                })
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

/// Whether `path` is a worktree of the repository checked out at `top`.
async fn is_worktree_of(path: &Path, top: &Path) -> bool {
    let common = ["rev-parse", "--path-format=absolute", "--git-common-dir"];
    let (Ok(ours), Ok(theirs)) = (git(path, &common).await, git(top, &common).await) else {
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

/// Whether the worktree at `path` holds nothing origin lacks: no change
/// left uncommitted, and no commit that no remote branch this clone knows
/// has.
async fn holds_nothing_origin_lacks(path: &Path) -> bool {
    let clean = git(path, &["status", "--porcelain"])
        .await
        .is_ok_and(|status| status.is_empty());
    let pushed = git(
        path,
        &["rev-list", "--max-count=1", "HEAD", "--not", "--remotes"],
    )
    .await
    .is_ok_and(|unpushed| unpushed.is_empty());
    clean && pushed
}

/// The file whose time says when a session last used `key`'s worktree.
pub(crate) fn marker(directory: &Path, key: &str) -> PathBuf {
    directory.join(format!("{key}.used"))
}

/// Say a session used `key`'s worktree at `at`.
fn touch(directory: &Path, key: &str, at: SystemTime) -> std::io::Result<()> {
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(marker(directory, key))?;
    file.set_modified(at)
}

/// When a session last used `key`'s worktree, as its marker says.
fn last_used(directory: &Path, key: &str) -> Option<SystemTime> {
    std::fs::metadata(marker(directory, key))
        .and_then(|metadata| metadata.modified())
        .ok()
}
