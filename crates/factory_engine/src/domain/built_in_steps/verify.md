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

End with `pass` when every Done-when clause holds, and with `fail` when one
does not or could not be checked. Hand back one line per clause, with
whether it holds and how you know; under each that does not, say what you
saw.
