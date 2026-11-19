use super::*;

const BASE: &str = "3333333333333333333333333333333333333333";

fn compared(paths: &[&str], complete: bool) -> CheckContext {
    CheckContext::compared(ComparedChanges {
        merge_base: BASE.to_string(),
        paths: paths.iter().map(|path| path.to_string()).collect(),
        complete,
    })
}

/// A full comparison's file says so on its first line, then lists each
/// path, a renamed file under both its names and a removed one too.
#[test]
fn a_complete_list_says_so_then_lists_each_path() {
    let context = compared(
        &[
            "apps/web/main.ts",
            "crates/old/lib.rs",
            "crates/new/lib.rs",
            "docs/removed.md",
        ],
        true,
    );

    assert_eq!(context.base, BASE);
    assert_eq!(
        context.file_text(),
        "complete\napps/web/main.ts\ncrates/old/lib.rs\ncrates/new/lib.rs\ndocs/removed.md\n"
    );
}

/// A comparison GitHub cut short lists what GitHub listed, under a first
/// line that says the list is partial.
#[test]
fn a_list_github_cut_short_says_it_is_partial() {
    let context = compared(&["a.rs", "b.rs"], false);

    assert_eq!(context.file_text(), "partial\na.rs\nb.rs\n");
}

/// One path per line cannot hold a path with a line break: it is left out,
/// and the list is partial.
#[test]
fn a_path_with_a_line_break_is_left_out_and_the_list_is_partial() {
    let context = compared(&["a.rs", "evil\ncomplete", "b\r.rs", "c.rs"], true);

    assert_eq!(context.file_text(), "partial\na.rs\nc.rs\n");
}

/// A list too long to send whole is cut, and says it is partial.
#[test]
fn a_list_over_its_size_is_cut_and_partial() {
    let long = "d/".repeat(1000);
    let paths: Vec<String> = (0..100).map(|n| format!("{long}{n}")).collect();
    let context = CheckContext::compared(ComparedChanges {
        merge_base: BASE.to_string(),
        paths,
        complete: true,
    });

    assert_eq!(context.listing, Listing::Partial);
    assert!(context.paths.len() < 100);
    assert!(context.file_text().len() <= MAX_LISTED_BYTES + "partial\n".len());
}

/// A failed comparison lists nothing and says the list is unavailable; the
/// base is the commit the Run is pinned to, or nothing.
#[test]
fn a_failed_comparison_is_unavailable() {
    let pinned = CheckContext::unavailable(Some(BASE));
    assert_eq!(pinned.file_text(), "unavailable\n");
    assert_eq!(pinned.base, BASE);

    assert_eq!(CheckContext::unavailable(None).base, "");
    assert_eq!(CheckContext::unavailable(Some("main; rm -rf /")).base, "");
}

/// A merge base that is no commit's id is not given.
#[test]
fn a_merge_base_that_is_no_commit_is_not_given() {
    let context = CheckContext::compared(ComparedChanges {
        merge_base: "main".to_string(),
        paths: Vec::new(),
        complete: true,
    });

    assert_eq!(context.base, "");
    assert_eq!(context.file_text(), "complete\n");
}
