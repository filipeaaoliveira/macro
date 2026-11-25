//! What a `cargo nextest` run said of its tests.

#[cfg(test)]
mod test;

/// How nextest's status lines begin for a test that did not pass: it
/// failed, ran out of time or died on a signal.
const NOT_PASSED: &[&str] = &[
    "FAIL", "TIMEOUT", "ABORT", "SIGSEGV", "SIGABRT", "SIGBUS", "SIGILL",
];

/// The tests nextest's output says did not pass, each named once, in the
/// order it first names them.
pub fn failed_tests(output: &str) -> Vec<String> {
    let mut failed: Vec<String> = Vec::new();
    for name in output.lines().filter_map(failed_test) {
        if !failed.iter().any(|seen| seen == name) {
            failed.push(name.to_owned());
        }
    }
    failed
}

/// The test a status line such as `FAIL [ 0.004s] (2/3) pkg::bin test::name`
/// names.
fn failed_test(line: &str) -> Option<&str> {
    let (status, rest) = line.trim_start().split_once(" [")?;
    if !NOT_PASSED.contains(&status) {
        return None;
    }

    let (_, test) = rest.split_once("] ")?;
    let test = test.trim_start();
    let test = match test.strip_prefix('(') {
        Some(counted) => counted.split_once(") ")?.1,
        None => test,
    };
    Some(test.trim())
}
