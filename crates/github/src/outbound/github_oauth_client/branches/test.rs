use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
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

#[tokio::test]
async fn a_commit_s_parents_are_read_first_first() {
    let (server, client) = answering(
        "200 OK",
        r#"{"sha":"m","parents":[{"sha":"base"},{"sha":"branch"}]}"#,
    );

    let parents = client
        .get_commit_parents("token", "macro", "app", "m")
        .await
        .unwrap();

    assert_eq!(parents, ["base", "branch"]);
    assert!(server.finish().starts_with("GET /repos/macro/app/commits/m HTTP/1.1"));
}

/// A commit contains another when it is that commit or comes after it,
/// never when it is behind or beside it.
#[tokio::test]
async fn a_commit_contains_those_it_comes_after() {
    for (status, contains) in [
        ("ahead", true),
        ("identical", true),
        ("behind", false),
        ("diverged", false),
    ] {
        let (server, client) = answering("200 OK", &format!(r#"{{"status":"{status}"}}"#));
        let read = client
            .commit_contains("token", "macro", "app", "merged", "later")
            .await
            .unwrap();
        assert_eq!(read, contains, "{status}");
        assert!(
            server
                .finish()
                .starts_with("GET /repos/macro/app/compare/merged...later?per_page=1 HTTP/1.1")
        );
    }
}
