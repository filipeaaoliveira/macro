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
- `src/domain/main_hold.rs` and `src/outbound/pg_run_repository/main_hold.rs`:
  `MainHoldRepository`, the merges Runs made and the Main Holds on the base
  branches they merge to.

## Scope

- Owns: the `factory_run`, `factory_step`, `factory_gate`,
  `factory_check_round`, `factory_merge` and `factory_main_hold` tables
  (`crates/macro_db_client/migrations/20261116083957_factory.sql`,
  `20261117093000_factory_check_round.sql`,
  `20261117104713_factory_step_agent.sql`,
  `20261117120425_factory_step_bot.sql`,
  `20261117170000_factory_turn_place.sql`,
  `20261117183000_factory_main_hold.sql`).
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
  its Role, the Agent the session is of and the bot that runs it
  (`bot_id`) once, in `claim_session`;
  `pass_session` records an Agent that could not start in `passed_over`, with
  its reason (`pass_reasons`), and replaces the session and Agent with the
  next one's when there is one, only while the Step still holds the session
  it replaces; with none, an Agent already recorded last is not recorded
  again. `record_prepare`, `record_open` and `holds_session` name the
  session too, so none acts on one the Step no longer works in. A session
  claimed before the Agent was recorded reads with none. The agent's latest
  hand back is kept on the Step while it runs (`hand_back`), and read when its
  turn ends (`handed_back`).
  A later prompt of the session (`begin_turn`) records its action in
  `turn_action_id` with its place among the Step's later prompts
  (`turn_place`, `20261117170000_factory_turn_place.sql`), replaces
  `offered` and clears the hand back, once, and only while the Step works in
  the session it names and took no prompt at that place or later: the same
  prompt again keeps what was handed back, and an earlier one is not
  recorded. Places order the prompts, never their ids, which workers make
  on their own clocks. It says
  whether the Step works on the prompt. `handed_back` names the prompt it
  reads for, and one the Step went on from reads as `Superseded`, never
  with a later prompt's answer. `pass_session` leaves a Step that took a
  later prompt alone, so its checks rounds stay in the session that did the
  work. `stopping_session` reads the latest prompt's action.
- A Step's rounds of checks (`factory_check_round`,
  `src/outbound/pg_run_repository/check_rounds.rs`) keep their first
  record, end and fix prompt, so a retried Step goes on with its round.
  `end_check_round_on_quota` keeps a round as ended by the agent's quota
  (`quota_ran_out`): no fix or round follows it.
  `list_steps` and `list_runs_steps` attach them to their Steps.
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

- `factory_merge` keeps one row per Run that merged (`record_merge`, safe
  to repeat), with the base it merged into and the time on the worker's
  clock; `latest_merge` reads a base's newest. `factory_main_hold` keeps one
  row per base branch: when its checks last passed (`last_green_at`),
  whether its required checks could not be read (`required_unread`, set by
  `mark_required_unread`, which drops any hold), and, while they fail, the
  hold (`held_since`, `head_sha`, `failed`). All its times are the reads'
  starts on the workers' clock, so they compare with the merges'.
  `hold_main` keeps a hold's start; `clear_main_hold` ends it and its red
  period, unless the hold started at or after the green read did.
  `suspect_merges` reads a base's merges since it was last green through
  `factory_merge_base_idx`, newest first up to its limit, and returns them
  oldest first. A red period files one task: `claim_hold_task` claims it for
  the hold's start once, or again once a claim with no task is older than
  `TASK_CLAIM_STALE_AFTER`; `release_hold_task` gives a claim back when
  filing failed, and `record_hold_task` keeps the task's id.

## Related

- Rules: `.agents/rules/rust/db.md` · Docs: `docs/development/database.md`
