
## Size block

Before the acceptance checks, write a fenced `json` block that says
whether the change fits one run. A plan reviewer reads it before a person
does, and the factory checks it.

When it fits, list each file the change touches, from the repository's
root (files, not folders), with `change` `add` for a new file, `edit` for
one the base holds, or `delete`; and map each Done-when item of the request
to the plan work that makes it hold, quoting the item as the request words
it:

```json
{"size": "fits",
 "files": [{"path": "src/slug.rs", "change": "edit"}, {"path": "src/slug/test.rs", "change": "add"}],
 "doneWhen": [{"item": "Slugs collapse repeated dashes", "work": "Step 2: collapse runs of dashes in slug(), tested in src/slug/test.rs"}]}
```

A change fits when one pull request can carry it and a reviewer can read
it in one sitting: at most about 20 files and 1000 changed lines, and no
parts that could merge on their own. When it does not, say why and split
the task into 2 to 8 child tasks, each a request another run can carry
out alone, with its own `## Done when` list. The factory files them and
this run ends there:

```json
{"size": "oversized", "why": "The API and the page can merge apart",
 "children": [{"name": "Add the slug API", "request": "Add ...\n\n## Done when\n\n- ..."},
              {"name": "Show slugs on the page", "request": "Show ...\n\n## Done when\n\n- ..."}]}
```

When the plan review raised blockers, revise the plan so each is resolved.
