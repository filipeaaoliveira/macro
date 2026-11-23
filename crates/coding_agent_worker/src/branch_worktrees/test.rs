use super::*;

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
    let worktrees = BranchWorktrees::new(repos.checkout.clone());

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
    let worktrees = BranchWorktrees::new(repos.checkout.clone());
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
    let worktrees = BranchWorktrees::new(repos.checkout.clone());
    let first = worktrees.prepare("factory/run").await.unwrap().unwrap();
    check_out(&first, "factory/run").await;
    commit_and_push(&first, "factory/run", "one").await;
    let pushed = git(&first, &["rev-parse", "HEAD"]).await.unwrap();
    remove(
        &repos.checkout,
        &repos.checkout.join(DIRECTORY),
        &key("factory/run").unwrap(),
    )
    .await;

    let again = worktrees.prepare("factory/run").await.unwrap().unwrap();

    assert_eq!(git(&again, &["rev-parse", "HEAD"]).await.unwrap(), pushed);
}

#[tokio::test]
async fn worktrees_no_session_used_for_long_or_whose_branch_origin_deleted_are_removed() {
    let repos = repositories().await;
    let worktrees = BranchWorktrees::new(repos.checkout.clone());
    let directory = repos.checkout.join(DIRECTORY);
    let stale = worktrees.prepare("factory/stale").await.unwrap().unwrap();
    let merged = worktrees.prepare("factory/merged").await.unwrap().unwrap();
    let waiting = worktrees.prepare("factory/waiting").await.unwrap().unwrap();
    check_out(&merged, "factory/merged").await;
    commit_and_push(&merged, "factory/merged", "merged").await;
    git(&repos.origin, &["branch", "-D", "factory/merged"])
        .await
        .unwrap();
    let age = |key: &str, by: Duration| {
        std::fs::File::options()
            .write(true)
            .open(marker(&directory, key))
            .unwrap()
            .set_modified(SystemTime::now() - by)
            .unwrap();
    };
    age("factory+stale", STALE_AFTER + Duration::from_secs(60));
    age("factory+merged", GONE_AFTER + Duration::from_secs(60));
    // Not pushed yet, and idle as long: its Run may wait on its owner.
    age("factory+waiting", GONE_AFTER + Duration::from_secs(60));

    worktrees.prepare("factory/other").await.unwrap().unwrap();

    assert!(!stale.exists());
    assert!(!merged.exists());
    assert!(waiting.exists());
    assert!(!marker(&directory, "factory+stale").exists());
    let listed = git(&repos.checkout, &["worktree", "list", "--porcelain"])
        .await
        .unwrap();
    assert!(!listed.contains("factory+stale"));
    assert!(!listed.contains("factory+merged"));
}

#[tokio::test]
async fn a_workspace_that_is_no_checkout_has_no_worktrees() {
    let root = tempfile::tempdir().unwrap();
    let worktrees = BranchWorktrees::new(root.path().to_owned());
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
        "a~1",
        "a:b",
    ] {
        assert!(key(branch).is_err(), "{branch:?} was accepted");
    }
}
