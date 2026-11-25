//! The lines a change made, from `git diff --unified=0 --no-prefix`.

use nom::Parser;
use nom::bytes::complete::tag;
use nom::character::complete::{char, u32 as number};
use nom::combinator::opt;
use nom::error::Error;
use nom::sequence::preceded;

#[cfg(test)]
mod test;

/// The lines one hunk left in the new file: `count` lines from `start`, or,
/// when `count` is 0, a removal just after line `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    /// The first line in the new file, or the line a removal follows.
    pub start: u32,
    /// How many lines of the new file it holds.
    pub count: u32,
}

impl Hunk {
    /// Whether the hunk changed a line of the function spanning `first` to
    /// `last`, both included. A removal changes it when lines are removed
    /// between two of its own.
    pub fn meets(self, first: u32, last: u32) -> bool {
        if self.count == 0 {
            return first <= self.start && self.start < last;
        }

        let end = self.start + self.count - 1;
        self.start <= last && first <= end
    }
}

/// One file the change left in the tree, and what it changed in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Its path from the repository's root.
    pub path: String,
    /// Its path at the base, when it was there: the earlier name of a
    /// renamed file. `None` for a new file.
    pub base_path: Option<String>,
    /// Its hunks, in order.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Whether the change touched the function spanning `first` to `last`.
    pub fn touches(&self, first: u32, last: u32) -> bool {
        self.hunks.iter().any(|hunk| hunk.meets(first, last))
    }
}

/// What `---` or `+++` names for a file that is not there.
const NO_FILE: &str = "/dev/null";

/// Reads a `git diff --unified=0 --no-prefix` output into the files it
/// leaves in the tree. Removed files are left out.
pub fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut base_path = None;

    for line in text.lines() {
        if let Some(old) = line.strip_prefix("--- ") {
            base_path = file_name(old);
        } else if let Some(new) = line.strip_prefix("+++ ") {
            start_file(&mut files, file_name(new), base_path.take());
        } else if let (Some(file), Some(hunk)) = (files.last_mut(), hunk_header(line)) {
            file.hunks.push(hunk);
        }
    }

    files.retain(|file| !file.path.is_empty());
    files
}

/// Starts the next file; a removed file gets an empty path, dropped at the
/// end, so its hunks have somewhere to go.
fn start_file(files: &mut Vec<FileDiff>, path: Option<String>, base_path: Option<String>) {
    files.push(FileDiff {
        path: path.unwrap_or_default(),
        base_path,
        hunks: Vec::new(),
    });
}

/// The path a `---` or `+++` line names. Git ends a name holding a space
/// with a tab.
fn file_name(name: &str) -> Option<String> {
    let name = name.trim_end_matches('\t');
    (name != NO_FILE).then(|| name.to_owned())
}

/// Reads `@@ -a[,b] +c[,d] @@` into the new file's side.
fn hunk_header(line: &str) -> Option<Hunk> {
    let mut header = (tag("@@ "), range('-'), char(' '), range('+'), tag(" @@"));
    let (_, (_, _, _, (start, count), _)) = header.parse(line).ok()?;
    Some(Hunk {
        start,
        count: count.unwrap_or(1),
    })
}

/// One side's `<sign>start[,count]`.
fn range<'a>(
    sign: char,
) -> impl Parser<&'a str, Output = (u32, Option<u32>), Error = Error<&'a str>> {
    (
        preceded(char(sign), number),
        opt(preceded(char(','), number)),
    )
}
