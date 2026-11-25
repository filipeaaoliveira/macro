use super::*;

const SOURCE: &str = "\
fn free() {
    let _ = 1;
}

struct Thing;

impl Thing {
    /// Docs are not part of the span.
    #[inline]
    pub fn method(&self) -> u8 {
        fn nested() {}
        nested();
        1
    }
}

trait Named {
    fn required(&self);

    fn provided(&self) -> u8 {
        2
    }
}
";

#[test]
fn each_function_spans_its_fn_keyword_to_its_closing_brace() {
    let spans = function_spans(SOURCE).unwrap();

    assert_eq!(
        spans,
        BTreeMap::from([(1, 3), (10, 14), (11, 11), (20, 22)])
    );
}

#[test]
fn a_file_that_does_not_parse_is_an_error() {
    assert!(function_spans("fn broken( {").is_err());
}
