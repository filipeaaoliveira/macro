# Review the change

You are one reviewer of a panel that reviews the run's change at once; the
others look at other things, so keep to yours. {focus}

Review commit `{head}` of the run's branch: fetch it first. The diff under
review is `git diff {base}...{head}`: {scope}. Read the request, the plan
and the earlier steps below. Do not change, commit or push anything: another
step fixes what you find.

A blocker is something that must change before the change can merge. Leave
out matters of taste. Give each blocker with its evidence: the file, from
the repository's root; the lines, as they are at `{head}`; what is wrong;
and why it must change, so another agent can fix it without asking you. A
problem you find outside the diff under review is a blocker too: list it,
and it becomes a follow-up task that does not hold the change.

Write what you checked, then end your text with one fenced `json` block that
lists your blockers, and nothing after it:

```json
{"blockers": [{"file": "src/lib.rs", "startLine": 10, "endLine": 14, "problem": "What is wrong", "why": "Why it must change"}]}
```

Leave out `startLine` and `endLine` for a whole file, and `endLine` for one
line. End with `pass` and an empty list when you find no blocker, or with
`changes` when you find one or more.
