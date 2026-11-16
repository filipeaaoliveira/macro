# factory_records

## What it is

Part of the software factory: keeps each Run of a Flow, the Steps it took and
the Gates they asked people in MacroDB, and defines the names Flows use
(`EntryId`, `Outcome`, `RunOutcome`).

## Start here

- `src/domain/models.rs`: `Run`, `Step`, `Gate`, their ends, and the name
  rules.
- `src/domain/ports.rs`: `RunRepository`, the port `factory_engine` writes through.
- `src/outbound/pg_run_repository.rs`: the `factory_run`, `factory_step` and
  `factory_gate` queries.

## Scope

- Owns: the `factory_run`, `factory_step` and `factory_gate` tables
  (migrations in `crates/macro_db_client/migrations/`).
- Not here: Flows, the interpreter, the workflow, the HTTP API → `crates/factory_engine`.

## Boundaries

- Depends on: `macro_user_id` (who answered a Gate).
- Used by: `crates/factory_engine`, `services/factory_service`.

## Checks

- `cargo test -p factory_records` (`SQLX_OFFLINE` unset, local Postgres migrated).

## Gotchas

- Every write a Step or a Run's end makes is repeated when Temporal retries
  its activity: `start_step` and `open_gate` ignore a conflict, and `end_run`
  and `answer_gate` keep the first end and answer. Keep new writes safe to
  repeat.
- `factory_run.task_id` references `"Document"` and is cleared when the task
  document is deleted; `link_task` reports a missing task or Run instead of
  failing.
- `answer_gate` runs after the workflow took the answer, so it must not fail on
  a user id with no account: it records no one instead.
- A Run's or Step's end columns are all null or all set (`factory_*_end`
  checks); the row mappings refuse anything else.

## Related

- Rules: `.agents/rules/rust/db.md` · Docs: `docs/development/database.md`
