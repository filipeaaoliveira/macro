//! The rows the check prints: one per touched function, joined from
//! `cargo crap`'s scores at the change and its complexities at the base.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::diff::FileDiff;

#[cfg(test)]
mod test;

/// The part of `cargo crap --format json`'s report the check reads.
#[derive(Debug, Deserialize)]
pub struct Report {
    /// One entry per scored function.
    pub entries: Vec<Entry>,
}

/// One function as `cargo crap` scores it.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    /// Its file: from the workspace root with `-p`, from `--path` without.
    pub file: String,
    /// Its name, with its type's for a method (`Type::method`).
    pub function: String,
    /// Its `fn` keyword's line.
    pub line: u32,
    /// Its cyclomatic complexity.
    pub cyclomatic: f64,
    /// The share of its lines the tests ran, in percent; none when the
    /// coverage holds nothing for its file.
    pub coverage: Option<f64>,
    /// Its CRAP score.
    pub crap: f64,
}

/// How a touched function's complexity compares with the base's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    /// The base has no function of its name in its file.
    New,
    /// More complex than at the base.
    Rose,
    /// As complex as at the base.
    Same,
    /// Less complex than at the base.
    Fell,
}

/// One touched function, as the check prints it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// Its file, from the repository's root.
    pub path: String,
    /// Its name.
    pub name: String,
    /// Its `fn` keyword's line.
    pub line: u32,
    /// Its cyclomatic complexity.
    pub complexity: f64,
    /// The share of its lines its package's tests ran, in percent.
    pub coverage: Option<f64>,
    /// Its CRAP score.
    pub score: f64,
    /// Against the base.
    pub change: Change,
    /// Its complexity at the base; none for a new function.
    pub base_complexity: Option<f64>,
    /// Whether it fails the check: new or more complex, and above the
    /// threshold.
    pub fails: bool,
}

/// A change in complexity smaller than this is none: complexities with a
/// fractional `?` weight are sums of floats.
const SAME_COMPLEXITY: f64 = 0.01;

/// A touched file: its diff and its functions' spans (last line by first).
#[derive(Debug, Clone)]
pub struct TouchedFile {
    /// What the change did to it.
    pub diff: FileDiff,
    /// Each function's last line, by its first.
    pub spans: BTreeMap<u32, u32>,
}

/// The rows of the functions the change touched, those that fail last and
/// the worst last among them, so the end of the output, which the checks
/// round keeps, holds what matters.
///
/// `current` holds the scores at the change, `base` the complexities at the
/// base, both with paths from the repository's root. A function is paired
/// with the base's function of the same name in the file's base path, the
/// n-th of a name with the n-th. Functions named `main` are left out: they
/// wire a binary up, and only running it exercises them.
pub fn rows(current: &[Entry], base: &[Entry], files: &[TouchedFile], threshold: f64) -> Vec<Row> {
    let mut rows: Vec<Row> = files
        .iter()
        .flat_map(|file| file_rows(current, base, file, threshold))
        .collect();

    rows.sort_by(|a, b| a.fails.cmp(&b.fails).then(a.score.total_cmp(&b.score)));
    rows
}

fn file_rows(current: &[Entry], base: &[Entry], file: &TouchedFile, threshold: f64) -> Vec<Row> {
    let mut at_base = base_complexities(base, file.diff.base_path.as_deref());
    let mut entries: Vec<&Entry> = current
        .iter()
        .filter(|entry| entry.file == file.diff.path)
        .collect();
    entries.sort_by_key(|entry| entry.line);

    // Every function takes its base pair, touched or not, so the n-th of a
    // name meets the n-th.
    entries
        .into_iter()
        .filter_map(|entry| {
            let base_complexity = at_base
                .get_mut(entry.function.as_str())
                .and_then(VecDeque::pop_front);
            let last = file.spans.get(&entry.line).copied().unwrap_or(entry.line);
            let touched = file.diff.touches(entry.line, last) && entry.function != "main";
            touched.then(|| row(entry, base_complexity, threshold))
        })
        .collect()
}

/// The base's complexities in `base_path`, by name, in line order.
fn base_complexities<'a>(
    base: &'a [Entry],
    base_path: Option<&str>,
) -> HashMap<&'a str, VecDeque<f64>> {
    let mut entries: Vec<&Entry> = base
        .iter()
        .filter(|entry| Some(entry.file.as_str()) == base_path)
        .collect();
    entries.sort_by_key(|entry| entry.line);

    let mut complexities: HashMap<&str, VecDeque<f64>> = HashMap::new();
    for entry in entries {
        complexities
            .entry(entry.function.as_str())
            .or_default()
            .push_back(entry.cyclomatic);
    }
    complexities
}

fn row(entry: &Entry, base_complexity: Option<f64>, threshold: f64) -> Row {
    let change = change(entry.cyclomatic, base_complexity);
    let riskier = matches!(change, Change::New | Change::Rose);
    Row {
        path: entry.file.clone(),
        name: entry.function.clone(),
        line: entry.line,
        complexity: entry.cyclomatic,
        coverage: entry.coverage,
        score: entry.crap,
        change,
        base_complexity,
        fails: riskier && entry.crap > threshold,
    }
}

fn change(complexity: f64, base_complexity: Option<f64>) -> Change {
    let Some(base) = base_complexity else {
        return Change::New;
    };

    if complexity > base + SAME_COMPLEXITY {
        Change::Rose
    } else if complexity < base - SAME_COMPLEXITY {
        Change::Fell
    } else {
        Change::Same
    }
}
