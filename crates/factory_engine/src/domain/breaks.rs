//! Breaks: how a write or fix Step's agent reports, for each test it adds,
//! an edit that breaks the code the test covers, and how the checks round
//! that runs it reads what happened.
//!
//! The agent reports each break as one line of what it hands back:
//!
//! ```text
//! BREAK <test>: <file> "<old text>" -> "<new text>" run `<command>`
//! ```
//!
//! `<file>` is a path from the repository's root; the break replaces the
//! first `<old text>` in it with `<new text>`, each on one line; `<command>`
//! runs the test from the repository's root, and holds no backtick. A later
//! line for the same test replaces an earlier one, so a fix can correct a
//! break. A line that starts with `BREAK` and reads as none is kept as
//! unreadable, so the reviewers see it.
//!
//! The round runs the test without the break, then with it, and puts the
//! file back ([`crate::domain::built_in_steps`]' `checks_round`): a break is
//! verified only when the test passed without it and failed with it.

#[cfg(test)]
mod test;

use factory_records::domain::breaks::{
    BreakStatus, MAX_BREAK_FIELD_BYTES, MAX_BREAK_OUTPUT_BYTES, NewBreakRecord,
};

/// What starts a break's line.
pub const BREAK_WORD: &str = "BREAK";

/// The most breaks one Step runs; any more are recorded dropped.
pub const MAX_BREAKS: usize = 20;

/// The most break lines, readable or not, one Step keeps records of; any
/// more are left out.
pub const MAX_REPORTED: usize = 50;

/// How a break's line is written, for a prompt or a reviewer.
pub const BREAK_FORM: &str =
    "BREAK <test>: <file> \"<old text>\" -> \"<new text>\" run `<command>`";

/// A break the agent reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Break {
    /// The test, as the agent names it.
    pub test: String,
    /// The file to edit, from the repository's root.
    pub file: String,
    /// The text whose first place in the file is replaced.
    pub old_text: String,
    /// What replaces it.
    pub new_text: String,
    /// The command that runs the test, from the repository's root.
    pub command: String,
}

/// A line that starts with [`BREAK_WORD`] and reads as no break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableBreak {
    /// The line, trimmed and cut to what a record keeps.
    pub line: String,
    /// Why it does not read.
    pub why: &'static str,
}

/// A break line, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reported {
    /// It reads.
    Break(Break),
    /// It does not.
    Unreadable(UnreadableBreak),
}

impl Reported {
    /// What the record of this break is keyed by: its test, or its line.
    pub fn test(&self) -> &str {
        match self {
            Reported::Break(read) => &read.test,
            Reported::Unreadable(line) => &line.line,
        }
    }

    /// Its record, with `status`, the commit it ran on and `output`; an
    /// unreadable line's is always [`BreakStatus::Unreadable`], saying why.
    pub fn record(
        &self,
        status: BreakStatus,
        checked_commit: Option<&str>,
        output: &str,
    ) -> NewBreakRecord {
        match self {
            Reported::Break(read) => NewBreakRecord {
                test: read.test.clone(),
                file: read.file.clone(),
                old_text: read.old_text.clone(),
                new_text: read.new_text.clone(),
                command: read.command.clone(),
                status,
                checked_commit: checked_commit.map(str::to_string),
                output: kept(output),
            },
            Reported::Unreadable(line) => NewBreakRecord {
                test: line.line.clone(),
                file: String::new(),
                old_text: String::new(),
                new_text: String::new(),
                command: String::new(),
                status: BreakStatus::Unreadable,
                checked_commit: None,
                output: format!("It does not read as `{BREAK_FORM}`: {}.", line.why),
            },
        }
    }
}

/// The breaks reported in `texts`, what a Step's agent handed back in
/// order: one per test, a later line for a test replacing an earlier one
/// where the earlier stood, and at most [`MAX_REPORTED`] tests.
pub fn reported<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<Reported> {
    let mut breaks: Vec<Reported> = Vec::new();
    for line in texts.into_iter().flat_map(str::lines) {
        let Some(read) = read_line(line) else {
            continue;
        };
        let full = breaks.len() >= MAX_REPORTED;
        match breaks.iter_mut().find(|kept| kept.test() == read.test()) {
            Some(kept) => *kept = read,
            None if !full => breaks.push(read),
            None => {}
        }
    }
    breaks
}

/// `line` as a break: none when it does not start with [`BREAK_WORD`],
/// once trimmed and any list mark taken off, or when it is the form the
/// prompt shows ([`BREAK_FORM`]).
pub fn read_line(line: &str) -> Option<Reported> {
    let trimmed = line.trim();
    let trimmed = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .unwrap_or(trimmed)
        .trim_start();
    let rest = trimmed.strip_prefix(BREAK_WORD)?;
    if !rest.starts_with(char::is_whitespace) || trimmed.contains("<test>: <file>") {
        return None;
    }
    let unreadable = |why| {
        Some(Reported::Unreadable(UnreadableBreak {
            line: cut(trimmed, MAX_BREAK_FIELD_BYTES).to_string(),
            why,
        }))
    };
    match read_break(rest.trim_start()) {
        Ok(read) => Some(Reported::Break(read)),
        Err(why) => unreadable(why),
    }
}

/// The part of a break's line after [`BREAK_WORD`], read. The command
/// ends the line; the test runs to the first `: ` that a file and a quoted
/// old text follow, so a test's id may hold `: `; the old and the new text
/// are split at their one `" -> "`, and a line whose texts hold it more
/// than once does not read, as no one could tell where the old text ends.
fn read_break(rest: &str) -> Result<Break, &'static str> {
    let (head, command) = rest
        .rsplit_once("\" run `")
        .ok_or("no run `<command>` follows the quoted new text")?;
    let command = command
        .strip_suffix('`')
        .ok_or("the command does not end with a backtick")?;
    if command.contains('`') {
        return Err("the command holds a backtick");
    }
    let command = command.trim();
    if command.is_empty() {
        return Err("the command is empty");
    }
    let (test, file, change) = head
        .match_indices(": ")
        .find_map(|(at, _)| {
            let after = head[at + 2..].trim_start();
            let (file, change) = after.split_once(" \"")?;
            (!file.is_empty() && !file.contains(char::is_whitespace))
                .then(|| (head[..at].trim(), file, change))
        })
        .ok_or("no `: <file> \"<old text>\"` follows the test")?;
    if test.is_empty() {
        return Err("it names no test");
    }
    let mut splits = change.match_indices("\" -> \"");
    let (at, _) = splits
        .next()
        .ok_or("no -> \"<new text>\" follows the old text")?;
    if splits.next().is_some() {
        return Err("the texts hold `\" -> \"` more than once");
    }
    let (old_text, new_text) = (&change[..at], &change[at + 6..]);
    if old_text.is_empty() {
        return Err("the old text is empty");
    }
    if old_text == new_text {
        return Err("the new text is the old text");
    }
    if !in_repository(file) {
        return Err("the file is not a path inside the repository");
    }
    let fields = [test, file, old_text, new_text, command];
    if fields
        .iter()
        .any(|field| field.len() > MAX_BREAK_FIELD_BYTES)
    {
        return Err("a part of it is longer than 1 KiB");
    }
    if fields
        .iter()
        .any(|field| field.chars().any(char::is_control))
    {
        return Err("it holds a control character");
    }
    Ok(Break {
        test: test.to_string(),
        file: file.to_string(),
        old_text: old_text.to_string(),
        new_text: new_text.to_string(),
        command: command.to_string(),
    })
}

/// Whether `file` is a relative path that stays inside the repository: no
/// `..` and no `.git` part, and not absolute.
fn in_repository(file: &str) -> bool {
    !file.is_empty()
        && !file.starts_with('/')
        && !file.contains('\\')
        && file.split('/').all(|part| {
            !part.is_empty() && part != ".." && part != "." && !part.eq_ignore_ascii_case(".git")
        })
}

/// The end of `text` a break's record keeps, at most
/// [`MAX_BREAK_OUTPUT_BYTES`], never split inside a character.
fn kept(text: &str) -> String {
    if text.len() <= MAX_BREAK_OUTPUT_BYTES {
        return text.to_string();
    }
    let mut start = text.len() - MAX_BREAK_OUTPUT_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

/// `text` cut to at most `max` bytes, never inside a character.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What a break's run on the agent's machine says
/// ([`crate::domain::built_in_steps`]' break command), read from the
/// markers it prints last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakRun {
    /// The file to break is not in the worktree, or lacks the old text.
    pub not_applied: bool,
    /// The test's exit status without the break.
    pub unbroken: Option<i32>,
    /// The test's exit status with it.
    pub broken: Option<i32>,
    /// Whether the file was put back as it was.
    pub restored: bool,
}

/// What begins each marker a break's run prints, before the run's nonce.
pub const MARKER: &str = "macro-break";

impl BreakRun {
    /// Read the markers of the run `nonce` from the run's standard output,
    /// or, when it holds none, from its standard error, where a runtime
    /// that merges the streams may return them.
    pub fn read_streams(stdout: &str, stderr: &str, nonce: &str) -> Self {
        let run = Self::read(stdout, nonce);
        if run == Self::read("", nonce) {
            return Self::read(stderr, nonce);
        }
        run
    }

    /// Read the markers of the run `nonce` that end `output`: the lines at
    /// its end that start with [`MARKER`] and the nonce, which the run
    /// prints last. Nothing before them counts, so nothing the test
    /// printed, which the run shows only the end of before its markers,
    /// reads as one, even with the nonce.
    pub fn read(output: &str, nonce: &str) -> Self {
        let prefix = format!("{MARKER} {nonce}:");
        let mut run = BreakRun {
            not_applied: false,
            unbroken: None,
            broken: None,
            restored: false,
        };
        let marked: Vec<&str> = output
            .trim_end()
            .lines()
            .rev()
            .map_while(|line| line.trim().strip_prefix(&prefix))
            .collect();
        for said in marked.into_iter().rev() {
            let said = said.trim();
            if said.starts_with("not-applied") {
                run.not_applied = true;
            } else if said == "restored" {
                run.restored = true;
            } else if let Some(code) = said.strip_prefix("unbroken ") {
                run.unbroken = code.trim().parse().ok();
            } else if let Some(code) = said.strip_prefix("broken ") {
                run.broken = code.trim().parse().ok();
            }
        }
        run
    }

    /// How the break went: verified only when the test passed without it
    /// and failed with it; interrupted when the run ended before it said
    /// how the test went.
    pub fn status(&self) -> BreakStatus {
        if self.not_applied {
            return BreakStatus::NotApplied;
        }
        match (self.unbroken, self.broken) {
            (Some(0), Some(0)) => BreakStatus::NotReproduced,
            (Some(0), Some(_)) => BreakStatus::Verified,
            (Some(0), None) | (None, _) => BreakStatus::Interrupted,
            (Some(_), _) => BreakStatus::NotRun,
        }
    }
}
