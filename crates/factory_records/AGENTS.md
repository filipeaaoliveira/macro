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
- `src/domain/insights.rs` and `src/outbound/pg_run_repository/insights.rs`:
  `RunInsightsRepository`, the reads behind Run insights.

## Scope

- Owns: the `factory_run`, `factory_step` and `factory_gate` tables
  (`crates/macro_db_client/migrations/20261116083957_factory.sql`,
  `20261117104713_factory_step_agent.sql`).
- Not here: Flows, the interpreter, the workflow, the HTTP API → `crates/factory_engine`.

## Boundaries

- Depends on: `macro_user_id` (who owns a Run and answered a Gate).
- Used by: `crates/factory_engine`, `services/factory_service`.

## Checks

- `cargo test -p factory_records` (`SQLX_OFFLINE` unset, local Postgres migrated).

## Gotchas

- Every write a Step or a Run's end makes is repeated when Temporal retries
  its activity: `start_step` and `open_gate` ignore a conflict, and `end_step`,
  `end_run` and `answer_gate` keep the first end and answer. Keep new writes
  safe to repeat.
- Steps and Gates are keyed by `(run_id, seq)`, the Step's place in its Run.
  `visit` is only the entry's count, which a reset restarts, so never key on
  it.
- `factory_run.user_id` is the Run's owner and goes with their account.
- An agent Step records its session (`session_id`, `action_id`, `offered`),
  its Role and the Agent the session is of once, in `claim_session`;
  `pass_session` replaces the session and Agent when the Agent could not
  start, appending it to `passed_over` with its reason (`pass_reasons`), and
  only while the Step still holds the session it replaces. The agent's latest
  hand back is kept on the Step while it runs (`hand_back`), and read when its
  turn ends (`handed_back`).
- `factory_run.task_id` references `"Document"` and is cleared when the task
  document is deleted; `link_task` reports a missing task or Run instead of
  failing. `list_task_runs` reads one owner's Runs on a task newest first
  through `factory_run_task_owner_idx` and stops at its limit: keep its
  `WHERE` and `ORDER BY` on that index's columns.
- `answer_gate` runs after the workflow took the answer, so it must not fail on
  an account deleted since: it records the answer with no author instead.
  The crate never reads `"User"`; who has an account is the authentication
  service's to say (`factory_engine`'s `UserDirectory`).
- `factory_run.flow` is the built-in Flow's name, which `insert_run` keeps;
  it is null only for Runs recorded before it was.
- `read_insights` reads one person's newest Runs in a period through
  `factory_run_user_created_idx` (`user_id, created_at DESC, id DESC`), up to
  its cap (one past it, to tell whether Runs were left out), then their Steps
  and Gates by primary key, in one read-only
  snapshot under a 5 s timeout. Keep its first `WHERE` and `ORDER BY` on that
  index, and every later read on the Runs' ids.
- A Run's or Step's end columns are all null or all set (`factory_*_end`
  checks); the row mappings refuse anything else.
- A Run that works on code keeps its repository and base branch together
  (`factory_run_repository` check), set once when it is kept. `set_merge_head`
  replaces the commit the Run asks a person to merge each time it asks; the
  merge reads it back, so it never merges another.

## Related

- Rules: `.agents/rules/rust/db.md` · Docs: `docs/development/database.md`
