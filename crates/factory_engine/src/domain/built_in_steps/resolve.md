# Resolve the conflict with the base

The run's base gained commits since its checks passed, and they conflict
with the change on the run's branch, so GitHub could not merge them.
Merge the base into the branch: fetch, check out the branch at
`origin/{branch}`, and run `git merge {base_sha}` (the base's head). Resolve
each conflict so that both the base's change and the run's change still do
what they did: keep what the base changed, and fit the run's change to it.
Change nothing else. Run the checks the plan names, commit the merge, and
push the branch, as the repository section below says. Never rebase, reset
or force-push: the branch keeps every commit it had.

End with `done` once the merge is pushed. When you cannot resolve a
conflict without guessing what someone meant, push nothing and end with
`conflict`, naming each file and what conflicts there, for a person to
resolve.
