use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
use super::CommitMade;
use crate::domain::models::GithubError;

fn answering(status: &'static str, body: &str) -> (MockServer, GithubOauthImpl) {
    let server = MockServer::start(MockResponse {
        status,
        body: body.to_string(),
    });
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());
    (server, client)
}

#[tokio::test]
async fn a_file_is_read_at_the_branch_s_head() {
    // `{"checks":[]}`, wrapped over two lines as GitHub sends it.
    let (server, client) = answering(
        "200 OK",
        r#"{"type":"file","encoding":"base64","content":"eyJjaGVj\na3MiOltdfQ==\n"}"#,
    );

    let text = client
        .get_file_text("token", "macro", "app", "factory/run", ".macro/checks.json")
        .await
        .unwrap();

    assert_eq!(text.as_deref(), Some(r#"{"checks":[]}"#));
    let request = server.finish();
    assert!(
        request.starts_with(
            "GET /repos/macro/app/contents/.macro/checks.json?ref=factory%2Frun HTTP/1.1"
        ),
        "{request}"
    );
}

/// A missing file, on a branch the user reaches, is none, and so is a
/// folder.
#[tokio::test]
async fn a_missing_file_or_a_folder_is_none() {
    let missing = MockServer::start_many(vec![
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Not Found"}"#.to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: r#"{"name":"main"}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(missing.base_url.clone());
    let text = client
        .get_file_text("token", "macro", "app", "main", ".macro/checks.json")
        .await
        .unwrap();
    assert_eq!(text, None);
    let requests = missing.finish_all();
    assert!(requests[1].starts_with("GET /repos/macro/app/branches/main HTTP/1.1"));

    let (folder, client) = answering("200 OK", r#"[{"type":"file","name":"checks.json"}]"#);
    let text = client
        .get_file_text("token", "macro", "app", "main", ".macro")
        .await
        .unwrap();
    assert_eq!(text, None);
    folder.finish();
}

/// A branch or repository out of reach is not a missing file, and neither
/// is an answer that does not read.
#[tokio::test]
async fn a_branch_out_of_reach_or_an_odd_answer_is_an_error() {
    let unreachable = MockServer::start_many(vec![
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Not Found"}"#.to_string(),
        },
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Branch not found"}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(unreachable.base_url.clone());
    let error = client
        .get_file_text("token", "macro", "app", "gone", ".macro/checks.json")
        .await
        .unwrap_err();
    assert!(matches!(error, GithubError::RepositoryUnavailable));
    unreachable.finish_all();

    let (odd, client) = answering("200 OK", r#"{"unexpected":true}"#);
    let read = client
        .get_file_text("token", "macro", "app", "main", ".macro/checks.json")
        .await;
    assert!(read.is_err());
    odd.finish();
}

#[tokio::test]
async fn changed_paths_name_both_sides_of_a_rename() {
    let (server, client) = answering(
        "200 OK",
        r#"{"files":[
            {"filename":"apps/web/main.ts","status":"modified"},
            {"filename":"crates/new/lib.rs","previous_filename":"crates/old/lib.rs","status":"renamed"}
        ]}"#,
    );

    let paths = client
        .list_changed_paths("token", "macro", "app", "main", "factory/run")
        .await
        .unwrap();

    assert_eq!(
        paths,
        Some(vec![
            "apps/web/main.ts".to_string(),
            "crates/old/lib.rs".to_string(),
            "crates/new/lib.rs".to_string(),
        ])
    );
    assert!(
        server
            .finish()
            .starts_with("GET /repos/macro/app/compare/main...factory/run HTTP/1.1")
    );
}

/// GitHub lists at most 300 files of a comparison: a list that long may be
/// cut, so the paths are not known.
#[tokio::test]
async fn a_comparison_github_cut_short_is_unknown() {
    let files: Vec<String> = (0..300)
        .map(|n| format!(r#"{{"filename":"file{n}.rs"}}"#))
        .collect();
    let (server, client) = answering("200 OK", &format!(r#"{{"files":[{}]}}"#, files.join(",")));

    let paths = client
        .list_changed_paths("token", "macro", "app", "main", "factory/run")
        .await
        .unwrap();

    assert_eq!(paths, None);
    server.finish();
}

/// A comparison gives its merge base and every path it changed, a renamed
/// file under both its names, the earlier first.
#[tokio::test]
async fn compared_changes_keep_the_merge_base_and_both_sides_of_a_rename() {
    let (server, client) = answering(
        "200 OK",
        r#"{"merge_base_commit":{"sha":"3333333333333333333333333333333333333333"},"files":[
            {"filename":"apps/web/main.ts","status":"modified"},
            {"filename":"docs/old.md","status":"removed"},
            {"filename":"crates/new/lib.rs","previous_filename":"crates/old/lib.rs","status":"renamed"}
        ]}"#,
    );

    let changes = client
        .compare_changes(
            "token",
            "macro",
            "app",
            "main",
            "1111111111111111111111111111111111111111",
        )
        .await
        .unwrap();

    assert_eq!(
        changes.merge_base,
        "3333333333333333333333333333333333333333"
    );
    assert_eq!(
        changes.paths,
        [
            "apps/web/main.ts",
            "docs/old.md",
            "crates/old/lib.rs",
            "crates/new/lib.rs",
        ]
    );
    assert!(changes.complete);
    assert!(server.finish().starts_with(
        "GET /repos/macro/app/compare/main...1111111111111111111111111111111111111111?per_page=1 HTTP/1.1"
    ));
}

/// A comparison GitHub cut at the 300 files it lists keeps the files it
/// listed, and says they are not all; one with no files listed at all, as
/// GitHub answers one too large, lists none and says so too. An empty list
/// is a comparison that changed nothing.
#[tokio::test]
async fn compared_changes_say_when_github_lists_only_part() {
    let files: Vec<String> = (0..300)
        .map(|n| format!(r#"{{"filename":"file{n}.rs"}}"#))
        .collect();
    let base = r#""merge_base_commit":{"sha":"3333333333333333333333333333333333333333"}"#;
    for (body, listed, complete) in [
        (
            format!(r#"{{{base},"files":[{}]}}"#, files.join(",")),
            300,
            false,
        ),
        (format!("{{{base}}}"), 0, false),
        (format!(r#"{{{base},"files":[]}}"#), 0, true),
    ] {
        let (server, client) = answering("200 OK", &body);

        let changes = client
            .compare_changes("token", "macro", "app", "main", "factory/run")
            .await
            .unwrap();

        assert_eq!(changes.paths.len(), listed, "{body}");
        assert_eq!(changes.complete, complete, "{body}");
        server.finish();
    }
}

/// A branch's head is the commit GitHub names for it; a branch the user
/// reaches none of is none.
#[tokio::test]
async fn a_branch_s_head_is_its_commit_or_none() {
    let (server, client) = answering(
        "200 OK",
        r#"{"name":"release/1.0","commit":{"sha":"2222222222222222222222222222222222222222"}}"#,
    );
    let head = client
        .get_branch_head("token", "macro", "app", "release/1.0")
        .await
        .unwrap();
    assert_eq!(
        head.as_deref(),
        Some("2222222222222222222222222222222222222222")
    );
    let request = server.finish();
    assert!(
        request.starts_with("GET /repos/macro/app/branches/release%2F1.0 HTTP/1.1"),
        "{request}"
    );

    let (gone, client) = answering("404 Not Found", r#"{"message":"Branch not found"}"#);
    let head = client
        .get_branch_head("token", "macro", "app", "main")
        .await
        .unwrap();
    assert_eq!(head, None);
    gone.finish();
}

#[tokio::test]
async fn compared_files_keep_their_hunks_and_a_comparison_cut_short_is_unknown() {
    let (server, client) = answering(
        "200 OK",
        r#"{"files":[
            {"filename":"src/lib.rs","status":"modified","patch":"@@ -1,2 +1,3 @@\n a\n+b\n c"},
            {"filename":"logo.png","previous_filename":"old.png","status":"renamed"}
        ]}"#,
    );
    let files = client
        .compare_files("token", "macro", "app", "abc123", "def456")
        .await
        .unwrap()
        .expect("listed in full");
    assert_eq!(files[0].path, "src/lib.rs");
    assert_eq!(
        files[0].patch.as_deref(),
        Some("@@ -1,2 +1,3 @@\n a\n+b\n c")
    );
    assert_eq!(files[1].previous_path.as_deref(), Some("old.png"));
    assert_eq!(files[1].patch, None);
    assert!(
        server
            .finish()
            .starts_with("GET /repos/macro/app/compare/abc123...def456 HTTP/1.1")
    );

    let many: Vec<String> = (0..300)
        .map(|n| format!(r#"{{"filename":"file{n}.rs"}}"#))
        .collect();
    let (cut, client) = answering("200 OK", &format!(r#"{{"files":[{}]}}"#, many.join(",")));
    let files = client
        .compare_files("token", "macro", "app", "main", "def456")
        .await
        .unwrap();
    assert_eq!(files, None);
    cut.finish();
}

/// A file read at a commit that holds none is none: the commit's own read,
/// not a branch's, tells a missing file from a commit out of reach.
#[tokio::test]
async fn a_missing_file_at_a_commit_is_none() {
    let commit = "3333333333333333333333333333333333333333";
    let missing = MockServer::start_many(vec![
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Not Found"}"#.to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: r#"{"sha":"3333333333333333333333333333333333333333","parents":[]}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(missing.base_url.clone());

    let text = client
        .get_file_text("token", "macro", "app", commit, ".macro/verify.json")
        .await
        .unwrap();

    assert_eq!(text, None);
    let requests = missing.finish_all();
    assert!(
        requests[1].starts_with(&format!("GET /repos/macro/app/commits/{commit} HTTP/1.1")),
        "{}",
        requests[1]
    );
}

/// A symlink GitHub follows to a file answers with that file's path: what
/// is at the path asked for is no file. A file over the 1 MB GitHub's
/// contents API gives comes with no content, which reads as too large.
#[tokio::test]
async fn a_followed_symlink_is_no_file_and_a_large_one_is_too_large() {
    let (linked, client) = answering(
        "200 OK",
        r#"{"type":"file","path":"elsewhere/rules.json","encoding":"base64","content":"e30="}"#,
    );
    let followed = client
        .get_file_text("token", "macro", "definition", "main", "rules.json")
        .await
        .unwrap();
    linked.finish();

    let (large, client) = answering(
        "200 OK",
        r#"{"type":"file","path":"agents.json","encoding":"none","content":""}"#,
    );
    let too_large = client
        .get_file_text("token", "macro", "definition", "main", "agents.json")
        .await;
    large.finish();

    assert_eq!(followed, None);
    let Err(GithubError::Internal(error)) = too_large else {
        panic!("a large file is an error: {too_large:?}");
    };
    assert!(
        error.downcast_ref::<super::FileTooLarge>().is_some(),
        "{error}"
    );
}

#[tokio::test]
async fn two_commits_merge_base_is_read_from_their_comparison() {
    let (server, client) = answering(
        "200 OK",
        r#"{"status":"diverged","merge_base_commit":{"sha":"3333333333333333333333333333333333333333"}}"#,
    );
    let main_head = "1111111111111111111111111111111111111111";
    let trial_head = "2222222222222222222222222222222222222222";
    let base = client
        .merge_base("token", "macro", "app", main_head, trial_head)
        .await
        .unwrap();
    assert_eq!(
        base.as_deref(),
        Some("3333333333333333333333333333333333333333")
    );
    assert!(server.finish().starts_with(&format!(
        "GET /repos/macro/app/compare/{main_head}...{trial_head}?per_page=1 HTTP/1.1"
    )));

    // Commits sharing no history, or one GitHub does not have: no base.
    let (unrelated, client) = answering("404 Not Found", r#"{"message":"No common ancestor"}"#);
    assert_eq!(
        client
            .merge_base("token", "macro", "app", main_head, trial_head)
            .await
            .unwrap(),
        None
    );
    unrelated.finish();

    // An id that is not a full commit id is never sent.
    let (_unused, client) = answering("200 OK", "{}");
    assert_eq!(
        client
            .merge_base("token", "macro", "app", "main", trial_head)
            .await
            .unwrap(),
        None
    );
}

/// An owner or repository name GitHub would not take is refused before any
/// request, as `..` would move the comparison off the repository.
#[tokio::test]
async fn merge_base_refuses_an_owner_or_repository_github_would_not_take() {
    // Nothing listens here: a request sent would fail as an outage.
    let client = GithubOauthImpl::with_api_base_url("http://127.0.0.1:9".to_string());
    let main_head = "1111111111111111111111111111111111111111";
    let trial_head = "2222222222222222222222222222222222222222";

    for (owner, repo) in [
        ("..", "app"),
        ("macro", "."),
        ("", "app"),
        ("macro/x", "app"),
        ("macro", "app?x"),
    ] {
        let refused = client
            .merge_base("token", owner, repo, main_head, trial_head)
            .await;
        assert!(
            matches!(refused, Err(GithubError::Internal(_))),
            "{owner:?}/{repo:?}: {refused:?}"
        );
    }
}

/// A commit is read with its parents in their order, its committer's
/// account and whether its signature is verified; a commit GitHub does not
/// have is none, and an id that is not a full commit id is never sent.
#[tokio::test]
async fn a_commit_is_read_with_its_parents_and_committer() {
    let merge = "3333333333333333333333333333333333333333";
    let (server, client) = answering(
        "200 OK",
        r#"{"sha":"3333333333333333333333333333333333333333","commit":{"verification":{"verified":true,"reason":"valid"}},"committer":{"login":"web-flow","id":19864447},"parents":[{"sha":"1111111111111111111111111111111111111111"},{"sha":"2222222222222222222222222222222222222222"}],"files":[]}"#,
    );
    let commit = client
        .commit_made("token", "macro", "app", merge)
        .await
        .unwrap();
    assert_eq!(
        commit,
        Some(CommitMade {
            parents: vec![
                "1111111111111111111111111111111111111111".to_string(),
                "2222222222222222222222222222222222222222".to_string(),
            ],
            committer_login: Some("web-flow".to_string()),
            verified: true,
        })
    );
    assert!(server.finish().starts_with(&format!(
        "GET /repos/macro/app/commits/{merge}?per_page=1 HTTP/1.1"
    )));

    // A commit whose committer's email belongs to no account, unsigned.
    let (server, client) = answering(
        "200 OK",
        r#"{"sha":"3333333333333333333333333333333333333333","commit":{"verification":{"verified":false,"reason":"unsigned"}},"committer":null,"parents":[{"sha":"1111111111111111111111111111111111111111"}]}"#,
    );
    assert_eq!(
        client
            .commit_made("token", "macro", "app", merge)
            .await
            .unwrap(),
        Some(CommitMade {
            parents: vec!["1111111111111111111111111111111111111111".to_string()],
            committer_login: None,
            verified: false,
        })
    );
    server.finish();

    let (missing, client) = answering("404 Not Found", r#"{"message":"Not Found"}"#);
    assert_eq!(
        client
            .commit_made("token", "macro", "app", merge)
            .await
            .unwrap(),
        None
    );
    missing.finish();

    let (_unused, client) = answering("200 OK", "{}");
    assert_eq!(
        client
            .commit_made("token", "macro", "app", "main")
            .await
            .unwrap(),
        None
    );
    let client = GithubOauthImpl::with_api_base_url("http://127.0.0.1:9".to_string());
    assert!(matches!(
        client.commit_made("token", "..", "app", merge).await,
        Err(GithubError::Internal(_))
    ));
}
