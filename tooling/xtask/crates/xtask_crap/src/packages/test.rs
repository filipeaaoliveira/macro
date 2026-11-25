use super::*;

const METADATA: &str = r#"{
    "workspace_root": "/repo",
    "packages": [
        {"name": "xtask", "manifest_path": "/repo/tooling/xtask/Cargo.toml"},
        {"name": "xtask_crap", "manifest_path": "/repo/tooling/xtask/crates/xtask_crap/Cargo.toml"},
        {"name": "factory_engine", "manifest_path": "/repo/crates/factory_engine/Cargo.toml"}
    ],
    "version": 1
}"#;

#[test]
fn each_file_belongs_to_the_package_whose_folder_holds_it_most_closely() {
    let metadata: Metadata = serde_json::from_str(METADATA).unwrap();
    let files = [
        "tooling/xtask/src/main.rs".to_owned(),
        "tooling/xtask/crates/xtask_crap/src/rows.rs".to_owned(),
        "crates/factory_engine/src/lib.rs".to_owned(),
        "crates/factory_engine/src/domain/checks.rs".to_owned(),
        "agent-teammates/muppet/src/lib.rs".to_owned(),
    ];

    assert_eq!(
        metadata.packages_of(&files),
        BTreeMap::from([
            (
                "factory_engine".to_owned(),
                vec![
                    "crates/factory_engine/src/lib.rs".to_owned(),
                    "crates/factory_engine/src/domain/checks.rs".to_owned(),
                ]
            ),
            (
                "xtask".to_owned(),
                vec!["tooling/xtask/src/main.rs".to_owned()]
            ),
            (
                "xtask_crap".to_owned(),
                vec!["tooling/xtask/crates/xtask_crap/src/rows.rs".to_owned()]
            ),
        ])
    );
}

#[test]
fn files_no_package_holds_are_outside() {
    let metadata: Metadata = serde_json::from_str(METADATA).unwrap();
    let files = [
        "crates/factory_engine/src/lib.rs".to_owned(),
        "apps/web/tauri/src/main.rs".to_owned(),
    ];

    assert_eq!(metadata.outside(&files), ["apps/web/tauri/src/main.rs"]);
}
