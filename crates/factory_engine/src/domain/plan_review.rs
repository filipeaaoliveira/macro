//! Plan review: before a person reads a plan at its Gate, the factory
//! checks it, then a plan reviewer reads it.
//!
//! On a repository the planner ends its plan with a size block, a fenced
//! `json` block before its acceptance checks:
//!
//! ```json
//! {"size": "fits",
//!  "files": [{"path": "src/lib.rs", "change": "edit"}, {"path": "src/new.rs", "change": "add"}],
//!  "doneWhen": [{"item": "Slugs collapse repeated dashes", "work": "step 2: collapse in slug()"}]}
//! ```
//!
//! or, for a task too big for one Run, the child tasks to split it into:
//!
//! ```json
//! {"size": "oversized", "why": "Three parts that merge on their own",
//!  "children": [{"name": "Add the API", "request": "... ## Done when ..."}, ...]}
//! ```
//!
//! The mechanical checks read a plan that fits: each file it edits or
//! deletes is at the Run's base, and each it adds is not; each Done-when
//! item of the request maps to plan work; and the Control Files it changes
//! are flagged for the plan reviewer and the person at the Gate. A Blocker
//! the checks find goes back to the planner without asking the reviewer.
//! An oversized plan goes to `split`, which files its child tasks.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use factory_records::domain::models::{RunId, Step};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::control_files::ControlFiles;
use crate::domain::ports::FollowUpTask;
use crate::domain::review_rounds::json_blocks;

/// The entry whose Step's output holds the plan, as the acceptance checks
/// read it.
const PLAN_ENTRY: &str = "plan";

/// The most files a plan that fits lists.
pub const MAX_PLANNED_FILES: usize = 40;

/// The fewest and most child tasks an oversized plan splits into.
pub const CHILDREN: std::ops::RangeInclusive<usize> = 2..=8;

/// The longest path, in bytes.
const MAX_PATH_BYTES: usize = 512;

/// The longest Done-when item, its plan work, or why a plan is oversized,
/// in bytes.
const MAX_TEXT_BYTES: usize = 2048;

/// The longest child task's name, in bytes.
const MAX_NAME_BYTES: usize = 200;

/// The longest child task's request, in bytes.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// The largest size block, in bytes, fences included: with the plan's
/// acceptance checks, it fits whole in what a Step's output holds.
pub const MAX_BLOCK_BYTES: usize = 16 * 1024;

/// The namespace of the ids of the child tasks a Run files.
const CHILD_NAMESPACE: Uuid = Uuid::from_u128(0x6f1c_2a54_93d8_4b07_a1e5_7c3b_d09e_4f21);

/// What a plan does to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// It adds the file, which the base does not hold.
    Add,
    /// It changes the file, which the base holds.
    Edit,
    /// It deletes the file, which the base holds.
    Delete,
}

impl FileChange {
    /// Whether the base holds the file before the change.
    pub fn at_base(self) -> bool {
        !matches!(self, FileChange::Add)
    }
}

/// A file a plan changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// From the repository's root.
    pub path: String,
    /// What the plan does to it.
    pub change: FileChange,
}

/// A Done-when item of the request, and the plan work that does it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedItem {
    /// The item, as the planner quoted it.
    pub item: String,
    /// The plan work that makes it hold.
    pub work: String,
}

/// A task an oversized plan splits off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildTask {
    /// Its name.
    pub name: String,
    /// What it asks, as markdown, with its own Done-when items.
    pub request: String,
}

/// What a plan's size block says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanSize {
    /// The change fits one Run.
    Fits {
        /// The files it changes.
        files: Vec<PlannedFile>,
        /// The request's Done-when items, each with its plan work.
        done_when: Vec<MappedItem>,
    },
    /// The task is too big for one Run: it is split into `children`.
    Oversized {
        /// Why.
        why: String,
        /// The child tasks, in order.
        children: Vec<ChildTask>,
    },
}

/// Why a plan's size block does not read: each is a Blocker.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SizeBlockError {
    /// The plan has no fenced `json` block naming `size`.
    #[error(
        "the plan has no size block: end it with a fenced json block naming `size`, before its acceptance checks"
    )]
    Missing,
    /// The block is larger than the plan's output keeps whole.
    #[error("the size block is over {MAX_BLOCK_BYTES} bytes: shorten the child tasks' requests")]
    TooLarge,
    /// The block is not a size block's JSON.
    #[error("the size block does not read: {0}")]
    Unreadable(String),
    /// `size` is neither `fits` nor `oversized`.
    #[error("the size block's size is {0:?}: it is `fits` or `oversized`")]
    Size(String),
    /// A plan that fits lists no file, or more than a plan lists.
    #[error(
        "the size block lists {0} files: a plan that fits lists 1 to {MAX_PLANNED_FILES}; a bigger change is `oversized`"
    )]
    Files(usize),
    /// A file's path or change does not read.
    #[error(
        "file {0} of the size block needs a `path` from the repository's root (no `..`, at most {MAX_PATH_BYTES} bytes) and a `change` of `add`, `edit` or `delete`"
    )]
    File(usize),
    /// A Done-when item or its work is blank or too long.
    #[error(
        "Done-when item {0} of the size block needs an `item` and its `work`, each 1 to {MAX_TEXT_BYTES} bytes"
    )]
    Item(usize),
    /// An oversized plan says not why, or names too few or too many
    /// children.
    #[error(
        "an oversized plan says `why` and splits into {min} to {max} `children`; it lists {0}",
        min = CHILDREN.start(),
        max = CHILDREN.end()
    )]
    Children(usize),
    /// A child task's name or request is blank or too long.
    #[error(
        "child task {0} needs a `name` of 1 to {MAX_NAME_BYTES} bytes and a `request` of 1 to {MAX_REQUEST_BYTES} bytes"
    )]
    Child(usize),
}

/// The size block as the planner writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WrittenSize {
    size: String,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    files: Vec<WrittenFile>,
    #[serde(default)]
    done_when: Vec<WrittenItem>,
    #[serde(default)]
    children: Vec<WrittenChild>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenFile {
    path: String,
    change: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenItem {
    item: String,
    work: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenChild {
    name: String,
    request: String,
}

impl PlanSize {
    /// What the size block of `plan` says: the last fenced `json` block
    /// that names `size`.
    pub fn from_plan(plan: &str) -> Result<Self, SizeBlockError> {
        let block = json_blocks(plan)
            .into_iter()
            .rev()
            .find(|block| block.names("size"))
            .ok_or(SizeBlockError::Missing)?;
        // The plan's output keeps a block of at most this size whole, however
        // long the plan's prose (`plan_output`).
        if block.end - block.start > MAX_BLOCK_BYTES {
            return Err(SizeBlockError::TooLarge);
        }
        let written: WrittenSize = serde_json::from_value(block.value)
            .map_err(|error| SizeBlockError::Unreadable(error.to_string()))?;

        match written.size.as_str() {
            "fits" => fits(written),
            "oversized" => oversized(written),
            other => Err(SizeBlockError::Size(other.to_string())),
        }
    }
}

/// A plan that fits, from its written block.
fn fits(written: WrittenSize) -> Result<PlanSize, SizeBlockError> {
    let count = written.files.len();
    if count == 0 || count > MAX_PLANNED_FILES {
        return Err(SizeBlockError::Files(count));
    }

    let mut files = Vec::new();
    for (index, file) in written.files.into_iter().enumerate() {
        let path = file
            .path
            .trim()
            .trim_start_matches("./")
            .trim_start_matches('/')
            .to_string();
        let change = match file.change.as_str() {
            "add" => FileChange::Add,
            "edit" => FileChange::Edit,
            "delete" => FileChange::Delete,
            _ => return Err(SizeBlockError::File(index + 1)),
        };
        if !plain_path(&path) {
            return Err(SizeBlockError::File(index + 1));
        }
        files.push(PlannedFile { path, change });
    }

    let mut done_when = Vec::new();
    for (index, item) in written.done_when.into_iter().enumerate() {
        let (item_text, work) = (item.item.trim(), item.work.trim());
        if !sized(item_text, MAX_TEXT_BYTES) || !sized(work, MAX_TEXT_BYTES) {
            return Err(SizeBlockError::Item(index + 1));
        }
        done_when.push(MappedItem {
            item: item_text.to_string(),
            work: work.to_string(),
        });
    }
    Ok(PlanSize::Fits { files, done_when })
}

/// An oversized plan, from its written block.
fn oversized(written: WrittenSize) -> Result<PlanSize, SizeBlockError> {
    let count = written.children.len();
    let why = written.why.unwrap_or_default().trim().to_string();
    if !CHILDREN.contains(&count) || !sized(&why, MAX_TEXT_BYTES) {
        return Err(SizeBlockError::Children(count));
    }

    let mut children = Vec::new();
    for (index, child) in written.children.into_iter().enumerate() {
        let (name, request) = (child.name.trim(), child.request.trim());
        let one_line = !name.contains('\n');
        if !one_line || !sized(name, MAX_NAME_BYTES) || !sized(request, MAX_REQUEST_BYTES) {
            return Err(SizeBlockError::Child(index + 1));
        }
        children.push(ChildTask {
            name: name.to_string(),
            request: request.to_string(),
        });
    }
    Ok(PlanSize::Oversized { why, children })
}

/// Whether `text` is not blank and at most `max` bytes.
fn sized(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max
}

/// Whether `path` names a file from the repository's root: not blank,
/// within its bound, each folder and the file named, none of them `.` or
/// `..`, in letters, digits and `.`, `_`, `-`, `+`, `@`, `[`, `]`, `(`, `)`
/// only. Nothing in it is read as part of a URL (`%`, `?`, `#`), quotes or
/// tags the text it is written into, or climbs out of the repository.
fn plain_path(path: &str) -> bool {
    let safe = path.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '/' | '.' | '_' | '-' | '+' | '@' | '[' | ']' | '(' | ')')
    });
    let named = path
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..");
    sized(path, MAX_PATH_BYTES) && safe && named
}

/// The entry of the `code` Flow whose Gate asks to approve a plan, and the
/// one that asked it before plans were reviewed.
const PLAN_GATE_ENTRIES: (&str, &str) = ("plan-review", "plan");

/// The entry that asked `entry`'s Gate before plans were reviewed: `plan`
/// for `plan-review`. An owner's answers there are past answers to it, and
/// a Gate recorded there is a plan Gate.
pub fn earlier_gate_entry(entry: &str) -> Option<&'static str> {
    (entry == PLAN_GATE_ENTRIES.0).then_some(PLAN_GATE_ENTRIES.1)
}

/// The entry that asks `entry`'s Gate now that plans are reviewed:
/// `plan-review` for `plan`.
pub fn later_gate_entry(entry: &str) -> Option<&'static str> {
    (entry == PLAN_GATE_ENTRIES.1).then_some(PLAN_GATE_ENTRIES.0)
}

/// How the output of a plan review starts when its last round still found
/// Blockers: the plan goes to its Gate with them.
pub const BLOCKERS_LEFT: &str = "The plan review still finds";

/// Whether `output`, a `plan_review` Step's, put the plan to its Gate with
/// Blockers left: then no Standing Rule and no stand-in approves it.
pub fn blockers_left(output: &str) -> bool {
    output.starts_with(BLOCKERS_LEFT)
}

/// The latest plan among a Run's `earlier` Steps: the output of its latest
/// `plan` Step that ended `done`.
pub fn latest_plan(earlier: &[Step]) -> Option<&str> {
    earlier
        .iter()
        .rev()
        .filter(|step| step.entry.as_str() == PLAN_ENTRY)
        .find_map(|step| {
            step.end
                .as_ref()
                .filter(|end| end.outcome.as_str() == "done")
                .map(|end| end.output.as_str())
        })
}

/// The Done-when items `request` lists: the items of the list under its
/// `Done when` heading (or a `Done when:` line), each as one line. None
/// when it has no such list.
pub fn done_when_items(request: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut under = false;
    for line in request.lines() {
        let trimmed = line.trim();
        if !under {
            under = is_done_when_heading(trimmed);
            continue;
        }

        // The list ends at the next heading, or at a line of prose once
        // it started.
        if trimmed.starts_with('#') {
            break;
        }
        if trimmed.is_empty() {
            continue;
        }
        if let Some(item) = list_item(trimmed) {
            items.push(item.to_string());
        } else if let Some(last) = items.last_mut().filter(|_| line.starts_with([' ', '\t'])) {
            // An indented line goes on with the item above it.
            last.push(' ');
            last.push_str(trimmed);
        } else if !items.is_empty() {
            break;
        }
    }
    items
}

/// Whether `line` is the heading of a request's Done-when list.
fn is_done_when_heading(line: &str) -> bool {
    let words = line
        .trim_start_matches('#')
        .trim_matches(|c: char| c == '*' || c == '_' || c == ':' || c.is_whitespace())
        .to_ascii_lowercase()
        .replace('-', " ");
    words == "done when"
}

/// The text of a list item, `- item`, `* item`, `+ item`, `1. item` or
/// `1) item`; none for any other line.
fn list_item(line: &str) -> Option<&str> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(item) = line.strip_prefix(bullet) {
            return Some(item.trim());
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    rest.strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
        .map(str::trim)
}

/// `text`'s words: lower case, every run of other characters one space.
fn words(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the planner's `quoted` item is the request's `item`: the same
/// words, or one holding the other where the shorter is at least half the
/// longer, as a quote that drops a clause's markup or its last words is.
fn same_item(quoted: &str, item: &str) -> bool {
    let (quoted, item) = (words(quoted), words(item));
    if quoted.is_empty() || item.is_empty() {
        return false;
    }
    let (short, long) = if quoted.len() <= item.len() {
        (&quoted, &item)
    } else {
        (&item, &quoted)
    };
    long.contains(short.as_str()) && short.len() * 2 >= long.len()
}

/// The Blockers of a plan's Done-when mapping: each of the request's
/// `items` no item of `mapped` quotes.
pub fn unmapped_items(items: &[String], mapped: &[MappedItem]) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| !mapped.iter().any(|done| same_item(&done.item, item)))
        .map(|(index, item)| {
            format!(
                "Done-when item {} (\"{}\") maps to no plan work: add it to the size block's `doneWhen` with the work that does it.",
                index + 1,
                item.replace('"', "'")
            )
        })
        .collect()
}

/// The Blockers of a plan's `files`, as `at_base` says which of them the
/// Run's base holds: an edit or a delete of a file it does not hold, and
/// an add of one it does. A path missing from `at_base` was not checked.
pub fn misplaced_files(files: &[PlannedFile], at_base: &BTreeMap<String, bool>) -> Vec<String> {
    files
        .iter()
        .filter_map(|file| {
            let held = *at_base.get(&file.path)?;
            match (file.change, held) {
                (FileChange::Edit | FileChange::Delete, false) => Some(format!(
                    "`{}` is not a file at the run's base, and the plan changes it: name the file the change is in, or mark it `add`.",
                    file.path
                )),
                (FileChange::Add, true) => Some(format!(
                    "`{}` is already a file at the run's base, and the plan adds it: mark it `edit`.",
                    file.path
                )),
                _ => None,
            }
        })
        .collect()
}

/// The flags of a plan's `files` that are Control Files: those `request`
/// names, when `request_asks`, and those it does not, which a person
/// approves at the merge.
pub fn control_file_flags(
    files: &[PlannedFile],
    control_files: &ControlFiles,
    request: &str,
    request_asks: bool,
) -> Vec<String> {
    let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
    let Some(changes) = control_files.changes(Some(&paths), request, request_asks) else {
        return Vec::new();
    };
    let mut flags = Vec::new();
    for path in changes.named {
        flags.push(format!(
            "`{path}` is a Control File the request names: the plan changes it as asked."
        ));
    }
    for path in changes.tampered {
        flags.push(format!(
            "`{path}` is a Control File the request does not name: the plan must say why it changes it, and a person approves the change before the merge."
        ));
    }
    flags
}

/// The id of the child task at `index` that the `split` Step at `seq` of
/// the Run `run_id` files: a UUIDv7 of the Run's own time, as document
/// storage takes a task's id, whose other bytes are fixed by the Run, the
/// Step and the child, so every attempt files the same task.
pub fn child_task_id(run_id: RunId, seq: u16, index: usize) -> Uuid {
    let name = format!("{run_id}:split:{seq}:{index}");
    let fixed = Uuid::new_v5(&CHILD_NAMESPACE, name.as_bytes());
    let mut bytes = [0u8; 10];
    bytes.copy_from_slice(&fixed.as_bytes()[6..16]);
    let millis = run_id
        .as_uuid()
        .get_timestamp()
        .map(|at| {
            let (seconds, nanos) = at.to_unix();
            seconds * 1000 + u64::from(nanos / 1_000_000)
        })
        .unwrap_or(0);
    uuid::Builder::from_unix_timestamp_millis(millis, &bytes).into_uuid()
}

/// A mention of the task document `id`, named `name`, as Macro's markdown
/// links one.
pub fn task_mention(id: Uuid, name: &str) -> String {
    let mention = serde_json::json!({
        "documentId": id.to_string(),
        "documentName": name,
        "blockName": "md",
        "blockParams": {},
    });
    format!("<m-document-mention>{mention}</m-document-mention>")
}

/// The task filed for `child`, split from the task `parent`, by its id and
/// name, when the Run `run_id` works on one.
pub fn child_task(child: &ChildTask, parent: Option<(Uuid, &str)>, run_id: RunId) -> FollowUpTask {
    let from = match parent {
        Some((parent, name)) => format!("Split from {} by", task_mention(parent, name)),
        None => "Split by".to_string(),
    };
    FollowUpTask {
        name: child.name.clone(),
        markdown: format!(
            "{}\n\n{from} factory Run {run_id}: its plan found the task too big for one run.\n",
            child.request
        ),
    }
}
