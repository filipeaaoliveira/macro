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
    let lasts: Vec<(u32, u32)> = functions(SOURCE)
        .unwrap()
        .into_iter()
        .map(|(first, function)| (first, function.last))
        .collect();

    assert_eq!(lasts, [(1, 3), (10, 14), (11, 11), (20, 22)]);
}

#[test]
fn a_file_that_does_not_parse_is_an_error() {
    assert!(functions("fn broken( {").is_err());
}

#[test]
fn bodies_that_differ_only_in_layout_and_comments_are_the_same() {
    let tight = functions("fn a() -> u8 { let x = 1; x }").unwrap();
    let loose = functions("fn b() -> u8 {\n    // one\n    let x = 1;\n\n    x\n}\n").unwrap();
    let other = functions("fn c() -> u8 { let x = 2; x }").unwrap();

    assert_eq!(tight[&1].body, loose[&1].body);
    assert_ne!(tight[&1].body, other[&1].body);
}
