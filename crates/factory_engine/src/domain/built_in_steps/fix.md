# Fix the change

You fix the change on the run's branch. When the latest review round found
blockers, the blockers section below lists each one with where it is, what is
wrong and why: fix each of them, and nothing the round did not ask for.
When the latest `accept` step below found that the change does not do what the
request asks, its report says which acceptance checks failed and which
Done-when clauses do not hold: make them hold, without changing the
acceptance checks, which the factory keeps and runs again. Otherwise the
latest `review` step below lists what must change, or, when a person answered
the question whether to merge with a note instead, their note says what to
change. Address each point, add or update tests, and run the
checks the plan names. Then commit and push the run's branch, as the
repository section below says.

For each test you add or change, and each test a blocker says has no break
or a break that did not fail, report a break: an edit to the code the test
covers that makes it fail, which you ran and then undid. Commit no break.
Give each as one line in the `text` you hand back with
`macro.factory.handBack`, not only in your chat reply; the factory reads
breaks from that text:

    BREAK <test>: <file> "<old text>" -> "<new text>" run `<command>`

`<file>` is a path from the repository's root, `<old text>` the text whose
first place in it the break replaces with `<new text>` (each on one line),
and `<command>` runs that test alone from the repository's root, with no
backtick in it. The factory runs each break again on the commit you push: the
test must pass without it and fail with it. A blocker that only asks for a
test's break needs no commit: its `BREAK` line answers it, and the next
review reads the new record on the same commit. Every other blocker you
fix needs a pushed commit.

End with `done` once the fix is pushed.
