# Run the repository's checks

The repository declares the checks below in `.macro/checks.json`. Run them on
the run's branch as you pushed it: in your checkout, at the latest commit of
the branch, from the repository's root, one after another, each exactly as
written. Change nothing, commit nothing and push nothing in this step, even
when a check fails: only report.

End with `passed` when every check exited with status 0, and with `failed`
when one did not, or could not run. Hand back one line per check, with its
name, its command and its exit status; under each check that failed, add the
last lines of its output that show why, at most 60 lines each.

## Checks

