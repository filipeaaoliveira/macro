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
    // The factory waits for no deploy: a file that names one does not read.
    assert!(matches!(
        VerifyFile::read(Some(
            r#"{"verify": false, "deploy": {"environment": "production"}}"#
        )),
        Err(VerifyFileError::Unreadable(_))
    ));
    assert_eq!(
        VerifyFile::read(Some(&" ".repeat(5000))),
        Err(VerifyFileError::TooLarge)
    );
}
