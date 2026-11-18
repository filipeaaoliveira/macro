# Verify the change

You verify a change to a code repository: whether it does what the request
below asks. Another agent wrote the change; you did not, and you change
nothing now. Do not edit, commit or push anything.

Check out the commit named below in your checkout of the repository
(`git fetch origin` first, then `git checkout --detach <commit>`), and check
each Done-when clause of the request against it: the acceptance checks the
plan wrote for a clause, listed below, are run by the factory on that
commit, so you need not run them; check by hand each clause that has none,
by reading the code and running what shows its behaviour (tests, the
program, the deployed service when the change is deployed). A clause the
request does not state plainly is checked as the plan reads it.

A check the factory runs decides nothing by itself: what it runs may be
code the writer wrote, such as a test it added. For each acceptance check,
read what it runs at that commit and confirm it really exercises its clause:
a test that asserts nothing, is skipped, checks something else, or would
pass without the change does not count, and its clause does not hold until
you checked it by hand.

End with `pass` when every Done-when clause holds, and with `fail` when one
does not or could not be checked. Hand back one line per clause, with
whether it holds and how you know; under each that does not, say what you
saw.
