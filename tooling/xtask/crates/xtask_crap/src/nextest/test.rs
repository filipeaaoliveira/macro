use super::*;

const OUTPUT: &str = "\
    Starting 4 tests across 2 binaries
        PASS [   0.004s] (1/4) xtask_crap diff::test::parses
        FAIL [   0.005s] (2/4) xtask_crap rows::test::pairs
  stdout ───
        TIMEOUT [ 180.002s] (3/4) xtask_crap::bin/xtask_crap test::hangs
        SIGSEGV [   0.010s] xtask_crap spans::test::crashes
     Summary [ 180.010s] 4 tests run: 1 passed, 3 failed
        FAIL [   0.005s] (2/4) xtask_crap rows::test::pairs
";

#[test]
fn every_test_that_did_not_pass_is_named_once() {
    assert_eq!(
        failed_tests(OUTPUT),
        [
            "xtask_crap rows::test::pairs",
            "xtask_crap::bin/xtask_crap test::hangs",
            "xtask_crap spans::test::crashes",
        ]
    );
}

#[test]
fn a_run_where_every_test_passed_names_none() {
    assert!(failed_tests("        PASS [   0.004s] (1/1) a b\n").is_empty());
}
