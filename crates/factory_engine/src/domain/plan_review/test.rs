use std::collections::BTreeMap;

use factory_records::domain::models::RunId;

use super::*;

/// A plan that fits, editing one file and adding another, with its two
/// Done-when items mapped.
const FITS: &str = r#"Collapse dashes in slugs.

```json
{"size": "fits",
 "files": [{"path": "src/slug.rs", "change": "edit"}, {"path": "./src/slug/test.rs", "change": "add"}],
 "doneWhen": [{"item": "Slugs collapse repeated dashes", "work": "Step 2"},
              {"item": "**A slug never ends with a dash.**", "work": "Step 3"}]}
```

```json
{"acceptance": [{"clause": "Slugs collapse repeated dashes", "run": "cargo test slug"}]}
```"#;

const REQUEST: &str = "Clean up slugs.\n\n## Done when\n\n- Slugs collapse repeated dashes.\n- A slug never ends\n  with a dash.\n\n## Notes\n\n- not an item\n";

#[test]
fn a_fitting_plan_reads_its_files_and_done_when_mapping() {
    let size = PlanSize::from_plan(FITS).expect("reads");

    let PlanSize::Fits { files, done_when } = size else {
        panic!("the plan fits");
    };
    assert_eq!(
        files,
        [
            PlannedFile {
                path: "src/slug.rs".to_string(),
                change: FileChange::Edit
            },
            PlannedFile {
                path: "src/slug/test.rs".to_string(),
                change: FileChange::Add
            },
        ]
    );
    assert_eq!(done_when.len(), 2);
}

#[test]
fn a_plan_without_a_size_block_or_with_a_bad_one_does_not_read() {
    let no_block = "Collapse dashes.\n\n```json\n{\"acceptance\": []}\n```";
    let bad_change = "```json\n{\"size\": \"fits\", \"files\": [{\"path\": \"a.rs\", \"change\": \"move\"}]}\n```";
    let climbs = "```json\n{\"size\": \"fits\", \"files\": [{\"path\": \"../a.rs\", \"change\": \"edit\"}]}\n```";
    let no_files = "```json\n{\"size\": \"fits\"}\n```";
    let unknown = "```json\n{\"size\": \"huge\"}\n```";

    assert_eq!(PlanSize::from_plan(no_block), Err(SizeBlockError::Missing));
    assert_eq!(
        PlanSize::from_plan(bad_change),
        Err(SizeBlockError::File(1))
    );
    assert_eq!(PlanSize::from_plan(climbs), Err(SizeBlockError::File(1)));
    assert_eq!(PlanSize::from_plan(no_files), Err(SizeBlockError::Files(0)));
    assert_eq!(
        PlanSize::from_plan(unknown),
        Err(SizeBlockError::Size("huge".to_string()))
    );
}

#[test]
fn an_oversized_plan_names_two_to_eight_children() {
    let plan = r#"Too big.

```json
{"size": "oversized", "why": "Two parts",
 "children": [{"name": "Add the API", "request": "Add it.\n\n## Done when\n\n- It answers"},
              {"name": "Show it", "request": "Show it.\n\n## Done when\n\n- It shows"}]}
```"#;
    let one_child = r#"```json
{"size": "oversized", "why": "One part", "children": [{"name": "All", "request": "All of it"}]}
```"#;

    let PlanSize::Oversized { why, children } = PlanSize::from_plan(plan).expect("reads") else {
        panic!("the plan is oversized");
    };
    assert_eq!(why, "Two parts");
    assert_eq!(
        children
            .iter()
            .map(|child| child.name.as_str())
            .collect::<Vec<_>>(),
        ["Add the API", "Show it"]
    );
    assert_eq!(
        PlanSize::from_plan(one_child),
        Err(SizeBlockError::Children(1))
    );
}

#[test]
fn the_request_s_done_when_items_are_its_list_under_the_heading() {
    assert_eq!(
        done_when_items(REQUEST),
        [
            "Slugs collapse repeated dashes.",
            "A slug never ends with a dash."
        ]
    );
    assert_eq!(
        done_when_items("Do it.\n\nDone when:\n1. It runs\n2) It stops\nThanks"),
        ["It runs", "It stops"]
    );
    assert!(done_when_items("Just do it.").is_empty());
}

#[test]
fn each_done_when_item_must_map_to_plan_work() {
    let items = done_when_items(REQUEST);
    let all = [
        MappedItem {
            item: "Slugs collapse repeated dashes".to_string(),
            work: "Step 2".to_string(),
        },
        MappedItem {
            item: "**A slug never ends with a dash.**".to_string(),
            work: "Step 3".to_string(),
        },
    ];
    let one = [MappedItem {
        item: "slugs collapse repeated dashes".to_string(),
        work: "Step 2".to_string(),
    }];
    let too_short = [MappedItem {
        item: "dash".to_string(),
        work: "Step 3".to_string(),
    }];

    assert!(unmapped_items(&items, &all).is_empty());
    let missing = unmapped_items(&items, &one);
    assert_eq!(missing.len(), 1);
    assert!(missing[0].starts_with("Done-when item 2 "), "{missing:?}");
    assert_eq!(unmapped_items(&items, &too_short).len(), 2);
}

#[test]
fn an_edit_needs_the_file_at_the_base_and_an_add_needs_it_absent() {
    let files = [
        PlannedFile {
            path: "src/slug.rs".to_string(),
            change: FileChange::Edit,
        },
        PlannedFile {
            path: "src/gone.rs".to_string(),
            change: FileChange::Delete,
        },
        PlannedFile {
            path: "src/new.rs".to_string(),
            change: FileChange::Add,
        },
        PlannedFile {
            path: "src/slugs.rs".to_string(),
            change: FileChange::Edit,
        },
        PlannedFile {
            path: "src/unknown.rs".to_string(),
            change: FileChange::Edit,
        },
    ];
    let at_base = BTreeMap::from([
        ("src/slug.rs".to_string(), true),
        ("src/gone.rs".to_string(), false),
        ("src/new.rs".to_string(), true),
        ("src/slugs.rs".to_string(), false),
    ]);

    let blockers = misplaced_files(&files, &at_base);

    // A path GitHub would not give (`src/unknown.rs`) is not checked.
    assert_eq!(blockers.len(), 3, "{blockers:?}");
    assert!(blockers[0].starts_with("`src/gone.rs` is not a file"));
    assert!(blockers[1].starts_with("`src/new.rs` is already a file"));
    assert!(blockers[2].starts_with("`src/slugs.rs` is not a file"));
}

#[test]
fn control_files_the_plan_changes_are_flagged() {
    let files = [
        PlannedFile {
            path: "src/slug.rs".to_string(),
            change: FileChange::Edit,
        },
        PlannedFile {
            path: ".github/workflows/ci.yml".to_string(),
            change: FileChange::Edit,
        },
        PlannedFile {
            path: "Cargo.toml".to_string(),
            change: FileChange::Edit,
        },
    ];
    let control_files = ControlFiles::parse(&ControlFiles::default_patterns()).expect("valid");

    let flags = control_file_flags(
        &files,
        &control_files,
        "Fix the CI job in .github/workflows/ci.yml",
        true,
    );

    assert_eq!(flags.len(), 2, "{flags:?}");
    assert!(flags[0].starts_with("`.github/workflows/ci.yml` is a Control File the request names"));
    assert!(flags[1].starts_with("`Cargo.toml` is a Control File the request does not name"));
}

#[test]
fn a_child_task_s_id_is_a_fixed_uuidv7_of_its_run_step_and_place() {
    let run = RunId::generate();

    let first = child_task_id(run, 3, 0);

    assert_eq!(first.get_version_num(), 7);
    assert_eq!(first, child_task_id(run, 3, 0));
    assert_ne!(first, child_task_id(run, 3, 1));
    assert_ne!(first, child_task_id(run, 4, 0));
    assert_eq!(
        first.get_timestamp().map(|at| at.to_unix().0),
        run.as_uuid().get_timestamp().map(|at| at.to_unix().0)
    );
}

#[test]
fn a_child_task_links_the_task_it_was_split_from() {
    let run = RunId::generate();
    let parent = Uuid::now_v7();
    let child = ChildTask {
        name: "Add the API".to_string(),
        request: "Add it.".to_string(),
    };

    let task = child_task(&child, Some((parent, "Clean up slugs")), run);

    assert_eq!(task.name, "Add the API");
    assert!(
        task.markdown
            .starts_with("Add it.\n\nSplit from <m-document-mention>")
    );
    assert!(task.markdown.contains(&parent.to_string()));
    assert!(task.markdown.contains(&run.to_string()));
}

#[test]
fn a_path_reads_only_in_its_safe_characters() {
    let block = |path: &str| {
        format!(
            "```json\n{{\"size\": \"fits\", \"files\": [{{\"path\": \"{path}\", \"change\": \"edit\"}}]}}\n```"
        )
    };

    for path in [
        "%2e%2e/secret",
        "a?.rs",
        "a#b.rs",
        "a b.rs",
        "a//b.rs",
        "a/./b.rs",
        "dir/",
    ] {
        assert_eq!(
            PlanSize::from_plan(&block(path)),
            Err(SizeBlockError::File(1)),
            "{path}"
        );
    }
    assert!(PlanSize::from_plan(&block("app/[slug]/page(1)@v2+x.tsx")).is_ok());
}

#[test]
fn a_size_block_too_large_to_keep_whole_is_a_blocker() {
    let request = "x".repeat(MAX_REQUEST_BYTES);
    let children: Vec<String> = (0..3)
        .map(|index| format!("{{\"name\": \"Part {index}\", \"request\": \"{request}\"}}"))
        .collect();
    let plan = format!(
        "```json\n{{\"size\": \"oversized\", \"why\": \"Big\", \"children\": [{}]}}\n```",
        children.join(", ")
    );

    assert_eq!(PlanSize::from_plan(&plan), Err(SizeBlockError::TooLarge));
}
