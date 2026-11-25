use super::*;

#[test]
fn ssh_and_https_urls_of_one_repository_share_a_folder() {
    assert_eq!(
        repository_folder("git@github.com:acme/permit-radar.git"),
        "github.com-acme-permit-radar"
    );
    assert_eq!(
        repository_folder("https://github.com/acme/permit-radar.git\n"),
        "github.com-acme-permit-radar"
    );
    assert_eq!(
        repository_folder("ssh://git@github.com/acme/permit-radar"),
        "github.com-acme-permit-radar"
    );
}

#[test]
fn credentials_in_a_url_are_left_out() {
    assert_eq!(
        repository_folder("https://x-access-token:secret@github.com/acme/app.git"),
        "github.com-acme-app"
    );
}

#[test]
fn a_path_becomes_one_folder_name() {
    assert_eq!(repository_folder("/home/ci/work/app"), "home-ci-work-app");
}
