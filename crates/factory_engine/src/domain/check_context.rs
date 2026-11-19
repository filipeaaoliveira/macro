//! What each check the factory runs itself is told about the change it
//! checks, so it can keep to what the Run changed: the worktree it runs in
//! may hold only the checked commit (a shallow clone fetches it at depth
//! 1), so git alone cannot say.
//!
//! - `MACRO_CHECK_BASE`: the commit the change is made against, the merge
//!   base GitHub's comparison of the Run's base (the commit a Benchmark
//!   trial is pinned to, else the base branch) with the checked commit
//!   gives. It is fetched into the repository when missing, at depth 1 in
//!   a shallow clone, as far as the machine can; empty when the comparison
//!   failed and the Run pins no commit.
//! - `MACRO_CHECK_CHANGED`: the absolute path of a file, outside the
//!   repository's tree (beside the round's worktree, in the machine user's
//!   cache), that lists the changed paths, in the format below. It is
//!   written afresh for each attempt and removed with the worktree.
//!
//! The file's first line says what the list holds, one of:
//!
//! - `complete`: every path the change touched;
//! - `partial`: the list, or the base, cannot be trusted whole: GitHub
//!   lists at most 300 files of a comparison and none of one too large to
//!   list; a path holding a line break, and paths past 64 KiB once quoted
//!   for the shell, are left out; or the base is not in the repository
//!   (it could not be fetched, or GitHub gave none), so a check cannot
//!   diff against it;
//! - `unavailable`: none, as the comparison failed.
//!
//! Each line after it is one path from the repository's root, as GitHub
//! names it, in GitHub's order. A renamed file is listed under both its
//! names, the earlier first; a removed file is listed though the worktree
//! no longer holds it. A check that keeps to the change must check
//! everything when the first line is not `complete`.

#[cfg(test)]
mod test;

/// The variable that names the commit the change is made against.
pub const BASE_VARIABLE: &str = "MACRO_CHECK_BASE";

/// The variable that names the file listing the changed paths.
pub const CHANGED_VARIABLE: &str = "MACRO_CHECK_CHANGED";

/// The most bytes the listed paths take in the command that writes the
/// file, each single-quoted for the shell (a `'` in a path takes four):
/// what follows is left out, and the list says it is partial. Sandboxes
/// run the command as one `bash -c` argument, which Linux holds to 128 KiB,
/// so the list keeps the whole command well short of that.
pub const MAX_QUOTED_BYTES: usize = 64 * 1024;

/// What `path` takes in the command that writes the file: single-quoted,
/// each `'` in it written `'\''`, and a space before it.
pub fn quoted_len(path: &str) -> usize {
    path.len() + 3 * path.matches('\'').count() + 3
}

/// What `head` changed against its merge base with a Run's base, as GitHub
/// compares them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparedChanges {
    /// The merge base: the commit the paths changed against.
    pub merge_base: String,
    /// The paths changed, a renamed file under both its names, the earlier
    /// first; as many as GitHub listed.
    pub paths: Vec<String>,
    /// Whether the paths are every change.
    pub complete: bool,
}

/// What the file's first line says of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// Every changed path.
    Complete,
    /// Only some of them.
    Partial,
    /// None: the comparison failed.
    Unavailable,
}

impl Listing {
    /// The file's first line.
    pub fn as_str(self) -> &'static str {
        match self {
            Listing::Complete => "complete",
            Listing::Partial => "partial",
            Listing::Unavailable => "unavailable",
        }
    }
}

/// What a round's checks are told of the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckContext {
    /// The commit the change is made against; empty when not known.
    pub base: String,
    /// What the list holds.
    pub listing: Listing,
    /// The paths listed.
    pub paths: Vec<String>,
}

impl CheckContext {
    /// The context of a comparison GitHub made: its merge base, and its
    /// paths, but for one holding a line break, which one path per line
    /// cannot hold, and those past [`MAX_QUOTED_BYTES`], either of which
    /// makes the list partial. A merge base that is not a commit's id is
    /// not given, and makes the list partial too: there is no commit to
    /// compare the worktree with.
    pub fn compared(changes: ComparedChanges) -> Self {
        let mut complete = changes.complete && is_commit(&changes.merge_base);
        let mut paths = Vec::new();
        let mut listed = 0;
        for path in changes.paths {
            if path.is_empty() || path.contains(['\n', '\r']) {
                complete = false;
                continue;
            }
            listed += quoted_len(&path);
            if listed > MAX_QUOTED_BYTES {
                complete = false;
                break;
            }
            paths.push(path);
        }
        Self {
            base: if is_commit(&changes.merge_base) {
                changes.merge_base
            } else {
                String::new()
            },
            listing: if complete {
                Listing::Complete
            } else {
                Listing::Partial
            },
            paths,
        }
    }

    /// The context when the comparison failed: no path, and the commit the
    /// Run is pinned to as its base, if it is.
    pub fn unavailable(pinned: Option<&str>) -> Self {
        Self {
            base: pinned
                .filter(|pinned| is_commit(pinned))
                .map_or_else(String::new, str::to_string),
            listing: Listing::Unavailable,
            paths: Vec::new(),
        }
    }

    /// The lines of the file: what the list holds, then each path.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.listing.as_str()).chain(self.paths.iter().map(String::as_str))
    }

    /// The file's text: [`Self::lines`], each ended by a line break.
    pub fn file_text(&self) -> String {
        self.lines().map(|line| format!("{line}\n")).collect()
    }
}

/// Whether `commit` names a commit as GitHub gives it: 40 or 64 lowercase
/// hex digits.
pub(crate) fn is_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
