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

End with `done` once the fix is pushed.
