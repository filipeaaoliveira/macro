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
-    let x = 1;
-    x
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
diff --git crates/a/src/moved.rs crates/b/src/moved.rs
similarity index 100%
rename from crates/a/src/moved.rs
rename to crates/b/src/moved.rs
";

#[test]
fn files_left_in_the_tree_keep_their_hunks_and_base_paths() {
    let files = parse(DIFF);

    assert_eq!(
        files,
        vec![
            FileDiff {
                path: "crates/a/src/lib.rs".to_owned(),
                base_path: Some("crates/a/src/lib.rs".to_owned()),
                hunks: vec![
                    Hunk { start: 3, count: 1 },
                    Hunk {
                        start: 11,
                        count: 4
                    },
                    Hunk {
                        start: 24,
                        count: 0
                    },
                ],
            },
            FileDiff {
                path: "crates/a/src/new.rs".to_owned(),
                base_path: None,
                hunks: vec![Hunk { start: 1, count: 2 }],
            },
            FileDiff {
                path: "crates/a/src/renamed.rs".to_owned(),
                base_path: Some("crates/a/src/old name.rs".to_owned()),
                hunks: vec![Hunk { start: 1, count: 1 }],
            },
        ]
    );
}

#[test]
fn a_hunk_meets_the_functions_whose_lines_it_holds() {
    let hunk = Hunk {
        start: 11,
        count: 4,
    };

    assert!(hunk.meets(14, 20));
    assert!(hunk.meets(5, 11));
    assert!(hunk.meets(12, 13));
    assert!(!hunk.meets(15, 20));
    assert!(!hunk.meets(1, 10));
}

#[test]
fn a_removal_meets_the_function_it_removed_lines_from() {
    let removal = Hunk {
        start: 24,
        count: 0,
    };

    assert!(removal.meets(20, 26));
    assert!(removal.meets(24, 25));
    assert!(!removal.meets(20, 24));
    assert!(!removal.meets(25, 30));
}
