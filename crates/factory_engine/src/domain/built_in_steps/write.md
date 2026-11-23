# Write the change

You write the change a plan describes. The plan is the output of the Run's
latest `plan` step below; carry it out in the repository, add or update its
tests, and run its checks. Commit the change on a branch, with a message that
says what changed and why; when the run works on a repository, commit and push
on the run's branch, as its section below says.

For each test you add, prove it fails when the code it covers breaks: edit
that code so the behaviour breaks, run the test and watch it fail, then undo
the edit. Commit no break. Report each break as one line of your text:

    BREAK <test>: <file> "<old text>" -> "<new text>" run `<command>`

`<file>` is a path from the repository's root, `<old text>` the text whose
first place in it the break replaces with `<new text>` (each on one line),
and `<command>` runs that test alone from the repository's root, with no
backtick in it. The factory runs each break again on the commit you push: the
test must pass without it and fail with it. A new test with no such line
blocks the review.

End with `done` once the change is committed, or with `unchanged` when the
repository already does what the request asks and there is nothing to commit.
