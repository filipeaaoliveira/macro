## Repository

The run works on the GitHub repository {repository}. Work in a checkout of
it: your working directory when it is one, or a clone of it made there. On a
machine that runs several runs at once, your working directory is a worktree
kept for this run alone: stay in it, and never check out a branch, commit or
change files in another checkout of the repository, where other runs work.
Run `git fetch origin` first. The run's branch is `{branch}` and its base is
`{base}`: check out `{branch}` at `origin/{branch}` when origin has it, and
otherwise create it from {start}. When git says `{branch}` is checked out in
another worktree, work on a detached `HEAD` at `origin/{branch}` instead, and
push with `git push origin HEAD:{branch}`.

When the step changes code, commit on `{branch}` with a message that says
what changed and why, and push it with `git push origin {branch}`. Never push
another branch, rewrite a commit you pushed, or force-push. When a step that writes
or fixes the change ends `done`, the factory reads the run's branch on
GitHub: with none of the step's work pushed there, the step fails.
