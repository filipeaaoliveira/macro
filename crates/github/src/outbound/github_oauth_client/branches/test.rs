use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};

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

#[tokio::test]
async fn a_missing_file_or_a_folder_is_none() {
    let (missing, client) = answering("404 Not Found", r#"{"message":"Not Found"}"#);
    let text = client
        .get_file_text("token", "macro", "app", "main", ".macro/checks.json")
        .await
        .unwrap();
    assert_eq!(text, None);
    missing.finish();

    let (folder, client) = answering("200 OK", r#"[{"type":"file","name":"checks.json"}]"#);
    let text = client
        .get_file_text("token", "macro", "app", "main", ".macro")
        .await
        .unwrap();
    assert_eq!(text, None);
    folder.finish();
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
