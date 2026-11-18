use super::{DeploySignal, VerifyFile, VerifyFileError};

/// A repository with no verify file is verified, with nothing deployed to
/// wait for.
#[test]
fn no_file_verifies_the_merged_commit() {
    assert_eq!(
        VerifyFile::read(None),
        Ok(VerifyFile {
            verify: true,
            deploy: None,
        })
    );
    assert_eq!(VerifyFile::read(Some("{}")), Ok(VerifyFile::default()));
}

#[test]
fn a_file_names_the_environment_a_merge_deploys_to() {
    let file = VerifyFile::read(Some(r#"{"deploy": {"environment": "production"}}"#));
    assert_eq!(
        file,
        Ok(VerifyFile {
            verify: true,
            deploy: Some(DeploySignal {
                environment: "production".to_string(),
            }),
        })
    );
}

#[test]
fn a_file_turns_the_phase_off() {
    let file = VerifyFile::read(Some(r#"{"verify": false}"#)).expect("reads");
    assert!(!file.verify);
}

#[test]
fn a_file_that_does_not_read_says_why() {
    assert!(matches!(
        VerifyFile::read(Some(r#"{"verify": "no"}"#)),
        Err(VerifyFileError::Unreadable(_))
    ));
    assert!(matches!(
        VerifyFile::read(Some(r#"{"deploy": {"workflow": "deploy.yml"}}"#)),
        Err(VerifyFileError::Unreadable(_))
    ));
    assert_eq!(
        VerifyFile::read(Some(r#"{"deploy": {"environment": "  "}}"#)),
        Err(VerifyFileError::Environment)
    );
    assert_eq!(
        VerifyFile::read(Some(&" ".repeat(5000))),
        Err(VerifyFileError::TooLarge)
    );
}
