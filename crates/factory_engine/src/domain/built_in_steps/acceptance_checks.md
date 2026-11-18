# Run the acceptance checks

Run the acceptance checks below on the commit named here, not on any branch:
in a worktree of your checkout at that commit (`git fetch origin <commit>`
when your checkout lacks it, then `git worktree add --detach <folder>
<commit>`), from the repository's root, one after another, each exactly as
written. Change nothing, commit nothing and push nothing, even when a check
fails: only report.

End with `passed` when every check exited with status 0, and with `failed`
when one did not, or could not run. Hand back one line per check, with its
name, its command and its exit status; under each check that failed, add the
last lines of its output that show why, at most 60 lines each.

