use super::*;

const OUTPUT: &str = "\
    Starting 7 tests across 2 binaries
        PASS [   0.004s] (1/7) xtask_crap diff::test::parses
        FAIL [   0.005s] (2/7) xtask_crap rows::test::pairs
  stdout ───
        TIMEOUT [ 180.002s] (3/7) xtask_crap::bin/xtask_crap test::hangs
        SIGSEGV [   0.010s] xtask_crap spans::test::crashes
  TRY 1 FAIL [   0.003s] (4/7) xtask_crap rows::test::flaky
  TRY 2 PASS [   0.003s] (4/7) xtask_crap rows::test::flaky
   LEAK-FAIL [   0.020s] (5/7) xtask_crap rows::test::leaks
     SIGKILL [   0.020s] (6/7) xtask_crap rows::test::killed
     Summary [ 180.010s] 7 tests run: 2 passed, 5 failed
        FAIL [   0.005s] (2/7) xtask_crap rows::test::pairs
";

#[test]
fn every_test_that_did_not_pass_is_named_once() {
    assert_eq!(
        failed_tests(OUTPUT),
        [
            "xtask_crap rows::test::pairs",
            "xtask_crap::bin/xtask_crap test::hangs",
            "xtask_crap spans::test::crashes",
            "xtask_crap rows::test::flaky",
            "xtask_crap rows::test::leaks",
            "xtask_crap rows::test::killed",
        ]
    );
}

#[test]
fn a_run_where_every_test_passed_names_none() {
    assert!(
        failed_tests("        PASS [   0.004s] (1/1) a b\n  TRY 2 PASS [ 0.1s] a c\n").is_empty()
    );
}
