# factory_service

## What it is

The software factory's service: serves the Run API and hosts the Temporal
worker that walks each Run's Flow, in one process.

## Start here

- `src/main.rs`: composition root — config, MacroDB pool, Temporal client and
  worker, router, shutdown.
- `src/config.rs`: `DATABASE_URL`, `INTERNAL_API_KEY`, `TEMPORAL_ADDRESS`.

## Scope

- Owns: wiring `factory_engine` and `factory_records` into a process.
- Not here: Flows, the workflow and the API's handlers → `crates/factory_engine`;
  the Run tables → `crates/factory_records`.
- Callers: internal services, with the internal API key. Users are refused.

## Checks

- `cargo build -p factory_service`
- `curl http://localhost:8113/health` after `just run_local`.
- A Run end to end: `POST http://localhost:8113/runs` with the
  `x-internal-auth-key` header (the stack's `INTERNAL_API_KEY`) and `{"flow": <crates/factory_engine/flows/reply.json>, "request": "..."}`,
  then `GET /runs/<id>`. `flows/plan.json` runs the agent Steps `plan` and
  `write`, with a Gate between them: answer it with
  `POST /runs/<id>/gates/plan/<visit>/answer` and
  `{"answer": "approve", "userId": "macro|<email>"}`. `POST /runs/<id>/cancel`
  cancels a Run.

## Gotchas

- The agent Steps run on `LocalAgent`, which does no work: no adapter to a
  real agent runtime is wired in yet.

- Local-only: the local stack runs a Temporal dev server (`temporal`, UI on
  `localhost:8233`); no deployed environment has Temporal yet, so the service
  has no deploy inventory, Doppler project or gateway route.
- The API and the worker stop together: when either stops, the process
  shuts the other down and exits.
