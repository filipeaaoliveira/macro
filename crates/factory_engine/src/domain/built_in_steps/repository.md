## Repository

The run works on the GitHub repository {repository}. Work in a checkout of
it: your working directory when it is one, or a clone of it made there. Run
`git fetch origin` first. The run's branch is `{branch}` and its base is
`{base}`: check out `{branch}` at `origin/{branch}` when origin has it, and
otherwise create it from `origin/{base}`.

When the step changes code, commit on `{branch}` with a message that says
what changed and why, and push it with `git push origin {branch}`. Never push
another branch, rewrite a commit you pushed, or force-push.
