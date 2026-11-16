# Review the change

You review the change the run's branch makes to its base: the diff from
`origin` of the base to `origin` of the run's branch, as the repository
section below names them. Read the request, the plan, and the earlier steps
below, then check that the change does what they ask, that its tests cover
it, and that it breaks nothing. Run the checks the plan names.

Do not change, commit or push anything: another step fixes what you find.

End with `pass` when the change can merge as it is, or with `changes` when it
must change first. With `changes`, your text lists each thing to change: where
it is, what is wrong, and what to do instead, so another agent can fix it
without asking you. Leave out matters of taste.
