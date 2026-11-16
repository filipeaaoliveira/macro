# factory_service

## What it is

The software factory's service: serves the Run API and hosts the Temporal
worker that walks each Run's Flow, in one process.

## Start here

- `src/main.rs`: composition root — config, MacroDB pool, Temporal client and
  worker, router, shutdown.
- `src/config.rs`: `DATABASE_URL`, `INTERNAL_API_KEY`, `TEMPORAL_ADDRESS`,
  `DOCUMENT_STORAGE_SERVICE_AUTH_KEY` (reading the tasks Runs work on),
  `AUTHENTICATION_SERVICE_SECRET_KEY` (telling which accounts exist),
  `FACTORY_AGENT_ID` (optional: the agent doing agent Steps).

## Scope

- Owns: wiring `factory_engine` and `factory_records` into a process.
- Not here: Flows, the workflow and the API's handlers → `crates/factory_engine`;
  the Run tables → `crates/factory_records`.
- Callers: internal services, with the internal API key and an acting user
  (`x-internal-macro-user-id`), who owns the Runs they create. Users are
  refused, but for an agent session's hand back.

## Checks

- `cargo build -p factory_service`
- `curl http://localhost:8113/health` after `just run_local`.
- A Run end to end: `POST http://localhost:8113/runs` with the
  `x-internal-auth-key` header (the stack's `INTERNAL_API_KEY`), the
  `x-internal-macro-user-id` header (`macro|<email>`), and
  `{"flow": <crates/factory_engine/flows/reply.json>, "request": "..."}`,
  then `GET /runs/<id>`. `flows/plan.json` runs the agent Steps `plan` and
  `write`, with a Gate between them: answer it with
  `POST /runs/<id>/steps/<seq>/answer` and `{"answer": "approve"}`.
  `POST /runs/<id>/cancel` cancels a Run. Add `"taskId": "<task document id>"` to link a Run to a task,
  or `PUT /runs/<id>/task` later.

## Gotchas

- `FACTORY_AGENT_ID` names the agent (a bot id) whose sessions do the agent
  Steps, as each Run's owner; the owner must be allowed to use it. Unset, the
  agent Steps run on `LocalAgent`, which does no work. Locally, set it in the
  `--env-file` the stack is started with (`docs/development/running-locally.md`). A macrod agent's
  sessions need its machine connected, and routine apps are refused for it.

- Local-only: the local stack runs a Temporal dev server (`temporal`, UI on
  `localhost:8233`); no deployed environment has Temporal yet, so the service
  has no deploy inventory, Doppler project or gateway route.
- The API and the worker stop together: when either stops, the process
  shuts the other down and exits.
