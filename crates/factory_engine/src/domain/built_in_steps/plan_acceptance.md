
## Acceptance checks

End the plan with a fenced `json` block of acceptance checks: one entry per
Done-when clause of the request (or, when it states none, per result it asks
for), each with the `clause` and either `run`, a command from the
repository's root that exits 0 only when the clause holds on the finished
change, or `none`, why no command can check it. A check exercises real
behaviour: a test you plan for the writer to add, a script, a request to the
running service. A verifier reads what each check runs before it counts, so
name the test and what it asserts in the plan. `$MACRO_VERIFY_COMMIT` names the commit under check. A check of
the deployed change is marked `"deployed": true`, and reaches it at
`$MACRO_DEPLOY_URL` when the deploy names where. `timeout` is how many
seconds a check may take, 600 when left out. The factory keeps these
checks: the writer cannot change them. Before the merge, the factory runs
every check not marked `deployed` on the run's branch, and a check that
fails goes back to be fixed; after the merge, it runs every check on the
merged change, and a check that fails fails the run.

```json
{"acceptance": [
  {"clause": "Slugs collapse repeated dashes", "run": "cargo test -p app slug_collapses_dashes"},
  {"clause": "The API serves the new field", "run": "curl -fs \"$MACRO_DEPLOY_URL/api/slug?q=a--b\" | grep -q a-b", "deployed": true},
  {"clause": "The settings page shows the new toggle", "none": "only a browser shows it"}
]}
```
