# factory_records

## What it is

Part of the software factory: keeps each Run of a Flow and the Steps it took in
MacroDB, and defines the names Flows use (`EntryId`, `Outcome`, `RunOutcome`).

## Start here

- `src/domain/models.rs`: `Run`, `Step`, their ends, and the name rules.
- `src/domain/ports.rs`: `RunRepository`, the port `factory_engine` writes through.
- `src/outbound/pg_run_repository.rs`: the `factory_run` and `factory_step` queries.

## Scope

- Owns: the `factory_run` and `factory_step` tables (migration in
  `crates/macro_db_client/migrations/`).
- Not here: Flows, the interpreter, the workflow, the HTTP API → `crates/factory_engine`.

## Boundaries

- Depends on: nothing of Macro's beyond `workspace-hack`.
- Used by: `crates/factory_engine`, `services/factory_service`.

## Checks

- `cargo test -p factory_records` (`SQLX_OFFLINE` unset, local Postgres migrated).

## Gotchas

- Every write a Step or a Run's end makes is repeated when Temporal retries
  its activity: `start_step` ignores a conflict and `end_run` keeps the first
  end. Keep new writes safe to repeat.
- A Run's or Step's end columns are all null or all set (`factory_*_end`
  checks); the row mappings refuse anything else.

## Related

- Rules: `.agents/rules/rust/db.md` · Docs: `docs/development/database.md`
