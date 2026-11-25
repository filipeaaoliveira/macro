//! The rows the check prints: one per touched function, joined from
//! `cargo crap`'s scores at the change and its complexities at the base.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::diff::FileDiff;
use crate::spans::Function;

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
    /// The base has no function it pairs with.
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
    /// Whether it would fail only because no test ran it while some tests
    /// failed: its coverage is not known, so it does not fail.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unproven: bool,
    /// Whether some tests failed, so its coverage may be lower than its
    /// tests would give.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub coverage_uncertain: bool,
}

/// A change in complexity smaller than this is none: complexities with a
/// fractional `?` weight are sums of floats.
const SAME_COMPLEXITY: f64 = 0.01;

/// A touched file: its diff and its functions.
#[derive(Debug, Clone)]
pub struct TouchedFile {
    /// What the change did to it.
    pub diff: FileDiff,
    /// Its functions, by first line.
    pub functions: BTreeMap<u32, Function>,
}

/// A file's version at the base, changed or removed since.
#[derive(Debug, Clone)]
pub struct BaseFile {
    /// Its path at the base.
    pub path: String,
    /// Its functions there, by first line.
    pub functions: BTreeMap<u32, Function>,
}

/// What decides a row's verdict.
#[derive(Debug, Clone, Copy)]
pub struct Verdict {
    /// The score a new or more complex function fails above.
    pub threshold: f64,
    /// Whether some tests failed while coverage was measured.
    pub tests_failed: bool,
}

/// The rows of the functions the change touched, those that fail last and
/// the worst last among them, so the end of the output, which the checks
/// round keeps, holds what matters.
///
/// `current` holds the scores at the change, `base` the complexities of
/// `base_files`, both with paths from the repository's root. A function
/// pairs with the base's function of its name whose line the diff carries
/// to its own; one left over pairs with one of its name left over in the
/// same file, in order; one still left over pairs with a base function no
/// function took, in any changed or removed file, whose body is the same:
/// it moved or was renamed. Functions named `main` are left out: they wire
/// a binary up, and only running it exercises them.
pub fn rows(
    current: &[Entry],
    base: &[Entry],
    files: &[TouchedFile],
    base_files: &[BaseFile],
    verdict: Verdict,
) -> Vec<Row> {
    let mut taken = vec![false; base.len()];
    let paired: Vec<Vec<(&Entry, Option<usize>)>> = files
        .iter()
        .map(|file| pair_in_file(current, base, &file.diff, &mut taken))
        .collect();
    let mut left_over = bodies_left_over(base, base_files, &taken);

    let mut rows = Vec::new();
    for (file, pairs) in files.iter().zip(paired) {
        for (entry, pair) in pairs {
            let function = file.functions.get(&entry.line);
            let pair = pair.or_else(|| moved_from(function, &mut left_over));
            if is_touched(entry, &file.diff, function) {
                rows.push(row(
                    entry,
                    pair.map(|index| base[index].cyclomatic),
                    verdict,
                ));
            }
        }
    }

    rows.sort_by(|a, b| {
        (a.fails, a.unproven)
            .cmp(&(b.fails, b.unproven))
            .then(a.score.total_cmp(&b.score))
    });
    rows
}

/// The file's functions with the index in `base` of each one's pair, by
/// line. Every function takes its pair, touched or not, so each base
/// function pairs once.
fn pair_in_file<'a>(
    current: &'a [Entry],
    base: &[Entry],
    diff: &FileDiff,
    taken: &mut [bool],
) -> Vec<(&'a Entry, Option<usize>)> {
    let mut entries: Vec<&Entry> = current
        .iter()
        .filter(|entry| entry.file == diff.path)
        .collect();
    entries.sort_by_key(|entry| entry.line);
    // In line order: cargo crap lists the worst first, and a function left
    // over pairs with the first of its name in the file.
    let mut candidates: Vec<usize> = (0..base.len())
        .filter(|&index| Some(base[index].file.as_str()) == diff.base_path.as_deref())
        .collect();
    candidates.sort_by_key(|&index| base[index].line);

    let mut pairs = pair_by_line(&entries, base, &candidates, diff, taken);
    pair_in_order(&entries, base, &candidates, &mut pairs, taken);
    entries.into_iter().zip(pairs).collect()
}

/// Pairs each base function with the function of its name the diff carries
/// its line to.
fn pair_by_line(
    entries: &[&Entry],
    base: &[Entry],
    candidates: &[usize],
    diff: &FileDiff,
    taken: &mut [bool],
) -> Vec<Option<usize>> {
    let mut pairs = vec![None; entries.len()];
    for &index in candidates {
        let at_base = &base[index];
        let Some(line) = diff.new_line(at_base.line) else {
            continue;
        };

        let found = entries
            .iter()
            .position(|entry| entry.line == line && entry.function == at_base.function);
        if let Some(position) = found.filter(|&position| pairs[position].is_none()) {
            pairs[position] = Some(index);
            taken[index] = true;
        }
    }
    pairs
}

/// Pairs each function left over with the first base function of its name
/// left over, in line order: one whose line the change rewrote.
fn pair_in_order(
    entries: &[&Entry],
    base: &[Entry],
    candidates: &[usize],
    pairs: &mut [Option<usize>],
    taken: &mut [bool],
) {
    for (entry, pair) in entries.iter().zip(pairs.iter_mut()) {
        if pair.is_some() {
            continue;
        }

        let found = candidates
            .iter()
            .copied()
            .find(|&index| !taken[index] && base[index].function == entry.function);
        if let Some(index) = found {
            *pair = Some(index);
            taken[index] = true;
        }
    }
}

/// The base functions no function took, by body, as indexes in `base`.
fn bodies_left_over(
    base: &[Entry],
    base_files: &[BaseFile],
    taken: &[bool],
) -> HashMap<String, Vec<usize>> {
    let mut bodies: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, entry) in base.iter().enumerate().filter(|(index, _)| !taken[*index]) {
        let function = base_files
            .iter()
            .find(|file| file.path == entry.file)
            .and_then(|file| file.functions.get(&entry.line));
        if let Some(function) = function {
            bodies.entry(function.body.clone()).or_default().push(index);
        }
    }
    bodies
}

/// The base function left over with the same body as `function`, taken.
fn moved_from(
    function: Option<&Function>,
    left_over: &mut HashMap<String, Vec<usize>>,
) -> Option<usize> {
    left_over.get_mut(&function?.body)?.pop()
}

/// Whether the change touched the function: a hunk meets its lines.
fn is_touched(entry: &Entry, diff: &FileDiff, function: Option<&Function>) -> bool {
    let last = function.map_or(entry.line, |function| function.last);
    entry.function != "main" && diff.touches(entry.line, last)
}

fn row(entry: &Entry, base_complexity: Option<f64>, verdict: Verdict) -> Row {
    let change = change(entry.cyclomatic, base_complexity);
    let riskier = matches!(change, Change::New | Change::Rose);
    let above = riskier && entry.crap > verdict.threshold;

    // With tests failing, an uncovered function may be one only those
    // tests run. It still fails when its complexity alone is above the
    // threshold: no coverage brings that down.
    let unproven = above
        && verdict.tests_failed
        && entry.coverage.unwrap_or(0.0) <= 0.0
        && entry.cyclomatic <= verdict.threshold;
    Row {
        path: entry.file.clone(),
        name: entry.function.clone(),
        line: entry.line,
        complexity: entry.cyclomatic,
        coverage: entry.coverage,
        score: entry.crap,
        change,
        base_complexity,
        fails: above && !unproven,
        unproven,
        coverage_uncertain: verdict.tests_failed,
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
