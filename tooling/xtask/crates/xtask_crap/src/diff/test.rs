use super::*;

const DIFF: &str = "\
diff --git crates/a/src/lib.rs crates/a/src/lib.rs
index 1111111..2222222 100644
--- crates/a/src/lib.rs
+++ crates/a/src/lib.rs
@@ -3 +3 @@ fn one() {
-    1
+    2
@@ -10,0 +11,4 @@ fn two() {
+fn three() {
+    3
+}
+
@@ -20,2 +24,0 @@ fn four() {
--- a removed comment line
--- /dev/null
diff --git crates/a/src/new.rs crates/a/src/new.rs
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ crates/a/src/new.rs
@@ -0,0 +1,2 @@
+fn new() {
+}
diff --git crates/a/src/gone.rs crates/a/src/gone.rs
deleted file mode 100644
index 4444444..0000000
--- crates/a/src/gone.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-fn gone() {
-}
diff --git crates/a/src/old name.rs crates/a/src/renamed.rs
similarity index 90%
rename from crates/a/src/old name.rs
rename to crates/a/src/renamed.rs
index 5555555..6666666 100644
--- crates/a/src/old name.rs\t
+++ crates/a/src/renamed.rs
@@ -1 +1 @@
-fn renamed() {}
+fn renamed() { }
+++ an added line that only looks like a header
diff --git crates/a/src/moved.rs crates/b/src/moved.rs
similarity index 100%
rename from crates/a/src/moved.rs
rename to crates/b/src/moved.rs
";

fn hunk(base_start: u32, base_count: u32, start: u32, count: u32) -> Hunk {
    Hunk {
        base_start,
        base_count,
        start,
        count,
    }
}

#[test]
fn files_left_in_the_tree_keep_their_hunks_and_base_paths() {
    let diff = parse(DIFF);

    assert_eq!(
        diff.files,
        vec![
            FileDiff {
                path: "crates/a/src/lib.rs".to_owned(),
                base_path: Some("crates/a/src/lib.rs".to_owned()),
                hunks: vec![hunk(3, 1, 3, 1), hunk(10, 0, 11, 4), hunk(20, 2, 24, 0)],
            },
            FileDiff {
                path: "crates/a/src/new.rs".to_owned(),
                base_path: None,
                hunks: vec![hunk(0, 0, 1, 2)],
            },
            FileDiff {
                path: "crates/a/src/renamed.rs".to_owned(),
                base_path: Some("crates/a/src/old name.rs".to_owned()),
                hunks: vec![hunk(1, 1, 1, 1)],
            },
        ]
    );
}

#[test]
fn removed_files_are_listed_by_their_base_path() {
    assert_eq!(parse(DIFF).removed, ["crates/a/src/gone.rs"]);
}

#[test]
fn a_hunk_meets_the_functions_whose_lines_it_holds() {
    let added = hunk(10, 0, 11, 4);

    assert!(added.meets(14, 20));
    assert!(added.meets(5, 11));
    assert!(added.meets(12, 13));
    assert!(!added.meets(15, 20));
    assert!(!added.meets(1, 10));
}

#[test]
fn a_removal_meets_the_function_it_removed_lines_from() {
    let removal = hunk(20, 2, 24, 0);

    assert!(removal.meets(20, 26));
    assert!(removal.meets(24, 25));
    assert!(!removal.meets(20, 24));
    assert!(!removal.meets(25, 30));
}

#[test]
fn a_base_line_moves_by_the_lines_added_and_removed_before_it() {
    let file = parse(DIFF).files.remove(0);

    assert_eq!(file.new_line(1), Some(1));
    assert_eq!(file.new_line(3), None);
    assert_eq!(file.new_line(5), Some(5));
    assert_eq!(file.new_line(10), Some(10));
    assert_eq!(file.new_line(11), Some(15));
    assert_eq!(file.new_line(20), None);
    assert_eq!(file.new_line(21), None);
    assert_eq!(file.new_line(22), Some(24));
}
