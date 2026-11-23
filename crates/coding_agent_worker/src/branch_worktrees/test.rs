use super::*;

use agent_client_protocol::LineDirection;

/// A repository on disk, `origin`, with one commit on `main`, and a clone
/// of it, the checkout sessions share, on `main`.
struct Repositories {
    _root: tempfile::TempDir,
    origin: PathBuf,
    checkout: PathBuf,
}

async fn repositories() -> Repositories {
    let root = tempfile::tempdir().unwrap();
    let seed = root.path().join("seed");
    std::fs::create_dir(&seed).unwrap();
    git(&seed, &["init", "--initial-branch=main"])
        .await
        .unwrap();
    identify(&seed).await;
    std::fs::write(seed.join("file"), "first").unwrap();
    git(&seed, &["add", "file"]).await.unwrap();
    git(&seed, &["commit", "-m", "initial"]).await.unwrap();
    let origin = root.path().join("origin.git");
    git(
        root.path(),
        &[
            "clone",
            "--bare",
            &seed.to_string_lossy(),
            &origin.to_string_lossy(),
        ],
    )
    .await
    .unwrap();
    let checkout = root.path().join("checkout");
    git(
        root.path(),
        &[
            "clone",
            &origin.to_string_lossy(),
            &checkout.to_string_lossy(),
        ],
    )
    .await
    .unwrap();
    identify(&checkout).await;
    let checkout = PathBuf::from(
        git(&checkout, &["rev-parse", "--show-toplevel"])
            .await
            .unwrap(),
    );
    Repositories {
        _root: root,
        origin,
        checkout,
    }
}

async fn identify(repository: &Path) {
    git(repository, &["config", "user.email", "test@example.com"])
        .await
        .unwrap();
    git(repository, &["config", "user.name", "Test"])
        .await
        .unwrap();
}

/// What an agent does on its Run's branch in `worktree`: check it out from
/// main, as its brief says when origin lacks it.
async fn check_out(worktree: &Path, branch: &str) {
    git(worktree, &["fetch", "origin"]).await.unwrap();
    git(worktree, &["checkout", "-b", branch, "origin/main"])
        .await
        .unwrap();
}

/// Commit `file` in `worktree`, and push nothing.
async fn commit_only(worktree: &Path, file: &str) -> String {
    std::fs::write(worktree.join(file), file).unwrap();
    git(worktree, &["add", file]).await.unwrap();
    git(worktree, &["commit", "-m", file]).await.unwrap();
    git(worktree, &["rev-parse", "HEAD"]).await.unwrap()
}

/// What an agent does once its change is written: commit `file` and push.
async fn commit_and_push(worktree: &Path, branch: &str, file: &str) {
    std::fs::write(worktree.join(file), branch).unwrap();
    git(worktree, &["add", file]).await.unwrap();
    git(worktree, &["commit", "-m", file]).await.unwrap();
    git(worktree, &["push", "origin", branch]).await.unwrap();
}

/// The subjects of the commits `branch` of `origin` has beyond `main`.
async fn subjects(origin: &Path, branch: &str) -> Vec<String> {
    git(origin, &["log", "--format=%s", &format!("main..{branch}")])
        .await
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn two_runs_at_once_commit_on_their_own_branches_and_leave_the_checkout_alone() {
    let repos = repositories().await;
    let head = git(&repos.checkout, &["rev-parse", "HEAD"]).await.unwrap();
    let worktrees = Arc::new(BranchWorktrees::new(
        repos.checkout.clone(),
        ActiveTurns::default(),
    ));

    let (first, second) = tokio::join!(
        worktrees.prepare("factory/run-1"),
        worktrees.prepare("factory/run-2")
    );
    let first = first.unwrap().expect("a worktree for the first run");
    let second = second.unwrap().expect("a worktree for the second run");
    assert_ne!(first, second);

    // The interleaving that put one Run's commit on the other's branch
    // when both worked in the one checkout.
    check_out(&first, "factory/run-1").await;
    check_out(&second, "factory/run-2").await;
    commit_and_push(&first, "factory/run-1", "one").await;
    commit_and_push(&second, "factory/run-2", "two").await;

    assert_eq!(subjects(&repos.origin, "factory/run-1").await, ["one"]);
    assert_eq!(subjects(&repos.origin, "factory/run-2").await, ["two"]);
    assert_eq!(
        git(&repos.checkout, &["symbolic-ref", "HEAD"])
            .await
            .unwrap(),
        "refs/heads/main"
    );
    assert_eq!(
        git(&repos.checkout, &["rev-parse", "HEAD"]).await.unwrap(),
        head
    );
    assert_eq!(
        git(&repos.checkout, &["status", "--porcelain"])
            .await
            .unwrap(),
        ""
    );
}

#[tokio::test]
async fn a_later_step_of_the_run_works_in_the_same_worktree() {
    let repos = repositories().await;
    let worktrees = Arc::new(BranchWorktrees::new(
        repos.checkout.clone(),
        ActiveTurns::default(),
    ));
    let first = worktrees.prepare("factory/run").await.unwrap().unwrap();
    check_out(&first, "factory/run").await;
    std::fs::write(first.join("unpushed"), "work").unwrap();

    let again = worktrees.prepare("factory/run").await.unwrap().unwrap();

    assert_eq!(again, first);
    assert!(again.join("unpushed").exists());
    assert_eq!(
        git(&again, &["symbolic-ref", "HEAD"]).await.unwrap(),
        "refs/heads/factory/run"
    );
}

#[tokio::test]
async fn a_new_worktree_starts_at_the_branch_origin_has() {
    let repos = repositories().await;
    let worktrees = Arc::new(BranchWorktrees::new(
        repos.checkout.clone(),
        ActiveTurns::default(),
    ));
    let first = worktrees.prepare("factory/run").await.unwrap().unwrap();
    check_out(&first, "factory/run").await;
    commit_and_push(&first, "factory/run", "one").await;
    let pushed = git(&first, &["rev-parse", "HEAD"]).await.unwrap();
    worktrees
        .remove(
            &repos.checkout,
            &directory(&repos.checkout),
            &key("factory/run").unwrap(),
            Keep::Nothing,
        )
        .await;

    let again = worktrees.prepare("factory/run").await.unwrap().unwrap();

    assert_eq!(git(&again, &["rev-parse", "HEAD"]).await.unwrap(), pushed);
}

/// Make a session of `worktrees` used `key`'s worktree `by` ago.
fn age(repos: &Repositories, key: &str, by: Duration) {
    touch(&directory(&repos.checkout), key, SystemTime::now() - by).unwrap();
}

#[tokio::test]
async fn worktrees_no_session_used_for_long_or_whose_branch_origin_deleted_are_removed() {
    let repos = repositories().await;
    let worktrees = Arc::new(BranchWorktrees::new(
        repos.checkout.clone(),
        ActiveTurns::default(),
    ));
    let stale = worktrees.prepare("factory/stale").await.unwrap().unwrap();
    let merged = worktrees.prepare("factory/merged").await.unwrap().unwrap();
    let empty = worktrees.prepare("factory/empty").await.unwrap().unwrap();
    let editing = worktrees.prepare("factory/editing").await.unwrap().unwrap();
    let recent = worktrees.prepare("factory/recent").await.unwrap().unwrap();
    check_out(&merged, "factory/merged").await;
    commit_and_push(&merged, "factory/merged", "merged").await;
    git(&repos.origin, &["branch", "-D", "factory/merged"])
        .await
        .unwrap();
    // Not pushed, with a change it would lose.
    std::fs::write(editing.join("draft"), "work").unwrap();
    let idle = GONE_AFTER + Duration::from_secs(60);
    age(
        &repos,
        "factory+stale",
        STALE_AFTER + Duration::from_secs(60),
    );
    age(&repos, "factory+merged", idle);
    age(&repos, "factory+empty", idle);
    age(&repos, "factory+editing", idle);

    worktrees.sweep("factory+other").await;

    assert!(!stale.exists());
    assert!(!merged.exists());
    assert!(!empty.exists(), "it held nothing origin lacks");
    assert!(editing.exists(), "it holds a change origin lacks");
    assert!(recent.exists());
    assert!(!marker(&directory(&repos.checkout), "factory+stale").exists());
    let listed = git(&repos.checkout, &["worktree", "list", "--porcelain"])
        .await
        .unwrap();
    assert!(!listed.contains("factory+stale"));
    assert!(!listed.contains("factory+merged"));
    assert!(
        git(
            &repos.checkout,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/heads/factory/merged"
            ]
        )
        .await
        .is_err(),
        "the removed worktree's branch is deleted with it"
    );
}

#[tokio::test]
async fn a_worktree_a_session_has_a_turn_running_in_is_never_removed() {
    let repos = repositories().await;
    let turns = ActiveTurns::default();
    let worktrees = Arc::new(BranchWorktrees::new(repos.checkout.clone(), turns.clone()));
    let path = worktrees.prepare("factory/busy").await.unwrap().unwrap();
    let cwd = serde_json::to_string(&path.to_string_lossy()).unwrap();
    turns.observe(
        &format!(r#"{{"jsonrpc":"2.0","id":1,"method":"session/new","params":{{"cwd":{cwd}}}}}"#),
        LineDirection::Stdin,
    );
    turns.observe(
        r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}"#,
        LineDirection::Stdout,
    );
    turns.observe(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"s1","prompt":[]}}"#,
        LineDirection::Stdin,
    );
    age(
        &repos,
        "factory+busy",
        STALE_AFTER + Duration::from_secs(60),
    );

    worktrees.sweep("factory+other").await;
    assert!(path.exists(), "a turn runs there");

    // The turn ends: the session used the worktree just now.
    turns.observe(
        r#"{"jsonrpc":"2.0","id":2,"result":{"stopReason":"end_turn"}}"#,
        LineDirection::Stdout,
    );
    worktrees.sweep("factory+other").await;
    assert!(path.exists(), "a turn ended there just now");
}

/// A Run waiting on its owner with a commit it never pushed keeps its
/// worktree and its branch, however long it waits, whether or not origin
/// lists the branch.
#[tokio::test]
async fn a_commit_never_pushed_keeps_its_worktree_and_branch_on_both_paths() {
    let repos = repositories().await;
    let worktrees = Arc::new(BranchWorktrees::new(
        repos.checkout.clone(),
        ActiveTurns::default(),
    ));
    let waiting = worktrees.prepare("factory/waiting").await.unwrap().unwrap();
    let old = worktrees.prepare("factory/old").await.unwrap().unwrap();
    check_out(&waiting, "factory/waiting").await;
    check_out(&old, "factory/old").await;
    let waiting_commit = commit_only(&waiting, "waiting").await;
    let old_commit = commit_only(&old, "old").await;
    // Not on origin, idle past the hour: the branch-gone path.
    age(&repos, "factory+waiting", GONE_AFTER + Duration::from_secs(60));
    // Idle past the days: the stale path.
    age(&repos, "factory+old", STALE_AFTER + Duration::from_secs(60));

    worktrees.sweep("factory+other").await;

    assert!(waiting.exists());
    assert!(old.exists());
    for (branch, commit) in [("factory/waiting", waiting_commit), ("factory/old", old_commit)] {
        assert_eq!(
            git(&repos.checkout, &["rev-parse", &format!("refs/heads/{branch}")])
                .await
                .unwrap(),
            commit
        );
    }
}

/// A worktree git cannot read when its branch's next Step opens is not
/// removed over the branch's unpushed commits: the prepare fails, and the
/// directory and the branch stay.
#[tokio::test]
async fn a_prepare_keeps_a_branch_with_unpushed_commits() {
    let repos = repositories().await;
    let worktrees = Arc::new(BranchWorktrees::new(
        repos.checkout.clone(),
        ActiveTurns::default(),
    ));
    let path = worktrees.prepare("factory/run").await.unwrap().unwrap();
    check_out(&path, "factory/run").await;
    let commit = commit_only(&path, "work").await;
    std::fs::remove_file(path.join(".git")).unwrap();

    let prepared = worktrees.prepare("factory/run").await;

    assert!(prepared.is_err(), "{prepared:?}");
    assert!(path.join("work").exists());
    assert_eq!(
        git(&repos.checkout, &["rev-parse", "refs/heads/factory/run"])
            .await
            .unwrap(),
        commit
    );
}

/// The first sweep removes the worktrees an earlier macrod kept in the
/// checkout itself, then the directory and its `info/exclude` line.
#[tokio::test]
async fn the_first_sweep_removes_the_worktrees_kept_in_the_checkout() {
    let repos = repositories().await;
    let legacy = repos.checkout.join(LEGACY_DIRECTORY);
    std::fs::create_dir(&legacy).unwrap();
    let old = legacy.join("factory+old");
    git(
        &repos.checkout,
        &["worktree", "add", "--detach", &old.to_string_lossy(), "HEAD"],
    )
    .await
    .unwrap();
    std::fs::write(marker(&legacy, "factory+old"), b"").unwrap();
    let exclude = repos.checkout.join(".git/info/exclude");
    let kept = std::fs::read_to_string(&exclude).unwrap_or_default();
    std::fs::write(&exclude, format!("{kept}/{LEGACY_DIRECTORY}/\n")).unwrap();
    let worktrees = BranchWorktrees::new(repos.checkout.clone(), ActiveTurns::default());

    worktrees.sweep("factory+other").await;

    assert!(!legacy.exists());
    assert!(
        !std::fs::read_to_string(&exclude)
            .unwrap()
            .contains(LEGACY_DIRECTORY)
    );
    assert_eq!(
        git(&repos.checkout, &["status", "--porcelain"]).await.unwrap(),
        ""
    );
}

#[tokio::test]
async fn a_workspace_that_is_no_checkout_has_no_worktrees() {
    let root = tempfile::tempdir().unwrap();
    let worktrees = Arc::new(BranchWorktrees::new(
        root.path().to_owned(),
        ActiveTurns::default(),
    ));
    assert!(worktrees.prepare("factory/run").await.unwrap().is_none());
}

#[test]
fn only_plain_branch_names_are_accepted() {
    assert_eq!(
        key("factory/0195f2a4-6a1c-7e33-9d7e-1f2b3c4d5e6f").unwrap(),
        "factory+0195f2a4-6a1c-7e33-9d7e-1f2b3c4d5e6f"
    );
    for branch in [
        "",
        "../escape",
        "factory/../main",
        "factory//run",
        "/factory",
        "factory/",
        "-x",
        ".hidden",
        "has space",
        "a+b",
        "run.lock",
        "run.used",
        "factory/Run",
        "a~1",
        "a:b",
    ] {
        assert!(key(branch).is_err(), "{branch:?} was accepted");
    }
}
