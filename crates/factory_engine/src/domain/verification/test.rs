use super::{VerifyFile, VerifyFileError};

/// A repository with no verify file is checked before its merge.
#[test]
fn no_file_checks_the_change() {
    assert_eq!(VerifyFile::read(None), Ok(VerifyFile { verify: true }));
    assert_eq!(VerifyFile::read(Some("{}")), Ok(VerifyFile::default()));
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
        VerifyFile::read(Some(r#"{"verify": false, "skip": true}"#)),
        Err(VerifyFileError::Unreadable(_))
    ));
    assert_eq!(
        VerifyFile::read(Some(&" ".repeat(5000))),
        Err(VerifyFileError::TooLarge)
    );
}

/// The deploy an earlier factory waited for after the merge is read and
/// ignored: a file that still names one keeps its `verify`.
#[test]
fn a_leftover_deploy_is_ignored() {
    let off = r#"{"verify": false, "deploy": {"environment": "production"}}"#;
    assert_eq!(
        VerifyFile::read(Some(off)),
        Ok(VerifyFile { verify: false })
    );
    assert_eq!(
        VerifyFile::read(Some(r#"{"deploy": {"environment": "production"}}"#)),
        Ok(VerifyFile::default())
    );
}
