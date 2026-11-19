# Bring the branch up to date with its base

The run's base gained commits since its checks passed, so the change was
never checked with them. GitHub merged the base, at commit `{base_sha}`,
into the run's branch: its head is now `{head}`. Fetch the branch and check
it out at that commit. Change nothing: the factory then runs the
repository's checks on it, and its pull request's checks run again.

End with `done` once the branch is checked out at `{head}`.
