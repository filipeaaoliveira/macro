//! The lines a change made, from `git diff --unified=0 --no-prefix`.

use nom::Parser;
use nom::bytes::complete::tag;
use nom::character::complete::{char, u32 as number};
use nom::combinator::opt;
use nom::error::Error;
use nom::sequence::preceded;

#[cfg(test)]
mod test;

/// One hunk: `base_count` lines from `base_start` in the base replaced by
/// `count` lines from `start` in the new file. A side with no lines names
/// the line the other side's lines follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    /// The first line in the base, or the line an addition follows.
    pub base_start: u32,
    /// How many lines of the base it holds.
    pub base_count: u32,
    /// The first line in the new file, or the line a removal follows.
    pub start: u32,
    /// How many lines of the new file it holds.
    pub count: u32,
}

impl Hunk {
    /// Whether the hunk changed a line of the function spanning `first` to
    /// `last` in the new file, both included. A removal changes it when
    /// lines are removed between two of its own.
    pub fn meets(self, first: u32, last: u32) -> bool {
        if self.count == 0 {
            return first <= self.start && self.start < last;
        }

        let end = self.start + self.count - 1;
        self.start <= last && first <= end
    }

    /// The last base line the hunk replaced.
    fn base_end(self) -> u32 {
        self.base_start + self.base_count - 1
    }

    /// Whether the hunk replaced the base's `line` or comes before it. An
    /// addition after line `base_start` comes before the lines after it.
    fn comes_before_or_holds(self, line: u32) -> bool {
        if self.base_count == 0 {
            self.base_start < line
        } else {
            self.base_start <= line
        }
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

    /// Where the base's line `line` is in the new file; none when the change
    /// replaced or removed it.
    pub fn new_line(&self, line: u32) -> Option<u32> {
        let mut shift = 0_i64;
        for hunk in self
            .hunks
            .iter()
            .take_while(|hunk| hunk.comes_before_or_holds(line))
        {
            if hunk.base_count > 0 && line <= hunk.base_end() {
                return None;
            }
            shift += i64::from(hunk.count) - i64::from(hunk.base_count);
        }
        u32::try_from(i64::from(line) + shift).ok()
    }
}

/// What a diff did: the files it left in the tree, and those it removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    /// The files in the new tree, added, changed or renamed.
    pub files: Vec<FileDiff>,
    /// The base paths of the files it removed.
    pub removed: Vec<String>,
}

/// What `---` or `+++` names for a file that is not there.
const NO_FILE: &str = "/dev/null";

/// Reads a `git diff --unified=0 --no-prefix` output.
///
/// A file's `---` and `+++` lines are read only in its header, between its
/// `diff --git` line and its first hunk, since a removed line `-- x` reads
/// `--- x` inside a hunk.
pub fn parse(text: &str) -> Diff {
    let mut reader = Reader::default();
    for line in text.lines() {
        reader.read(line);
    }
    reader.diff
}

/// Where [`parse`] is in the diff.
#[derive(Default)]
struct Reader {
    diff: Diff,
    /// Between a `diff --git` line and the file's first hunk.
    in_header: bool,
    /// The file's `---` path.
    base_path: Option<String>,
    /// Whether the file is in the new tree, so its hunks are kept.
    kept: bool,
}

impl Reader {
    fn read(&mut self, line: &str) {
        if line.starts_with("diff --git ") {
            self.in_header = true;
            self.kept = false;
        } else if let Some(hunk) = hunk_header(line) {
            self.in_header = false;
            self.add_hunk(hunk);
        } else if self.in_header {
            self.read_header(line);
        }
    }

    fn read_header(&mut self, line: &str) {
        if let Some(old) = line.strip_prefix("--- ") {
            self.base_path = file_name(old);
        } else if let Some(new) = line.strip_prefix("+++ ") {
            self.start_file(file_name(new));
        }
    }

    /// Starts the next file, or lists it as removed.
    fn start_file(&mut self, path: Option<String>) {
        let base_path = self.base_path.take();
        self.kept = path.is_some();
        match path {
            Some(path) => self.diff.files.push(FileDiff {
                path,
                base_path,
                hunks: Vec::new(),
            }),
            None => self.diff.removed.extend(base_path),
        }
    }

    fn add_hunk(&mut self, hunk: Hunk) {
        if let Some(file) = self.diff.files.last_mut().filter(|_| self.kept) {
            file.hunks.push(hunk);
        }
    }
}

/// The path a `---` or `+++` line names. Git ends a name holding a space
/// with a tab.
fn file_name(name: &str) -> Option<String> {
    let name = name.trim_end_matches('\t');
    (name != NO_FILE).then(|| name.to_owned())
}

/// Reads `@@ -a[,b] +c[,d] @@`.
fn hunk_header(line: &str) -> Option<Hunk> {
    let mut header = (tag("@@ "), range('-'), char(' '), range('+'), tag(" @@"));
    let (_, (_, (base_start, base_count), _, (start, count), _)) = header.parse(line).ok()?;
    Some(Hunk {
        base_start,
        base_count: base_count.unwrap_or(1),
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
