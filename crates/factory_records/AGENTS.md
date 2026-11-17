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
- `src/domain/review_rounds.rs` and
  `src/outbound/pg_run_repository/review_rounds.rs`: `ReviewRoundRepository`,
  a Run's review rounds and what their reviewers found.
- `src/domain/insights.rs` and `src/outbound/pg_run_repository/insights.rs`:
  `RunInsightsRepository`, the reads behind Run insights.
- `src/domain/main_hold.rs` and `src/outbound/pg_run_repository/main_hold.rs`:
  `MainHoldRepository`, the merges Runs made and the Main Holds on the base
  branches they merge to.
- `src/domain/standing_rules.rs` and
  `src/outbound/pg_run_repository/standing_rules.rs`:
  `StandingRulesRepository`, teams' and Projects' Standing Rules, and the
  Gate kinds, rules and Orchestrator Definition they are made of.

## Scope

- Owns: the `factory_run`, `factory_step`, `factory_gate`,
  `factory_check_round`, `factory_team_settings`, `factory_merge`,
  `factory_main_hold`, `factory_review_round`, `factory_review_finding` and
  `factory_project_rules` tables
  (`crates/macro_db_client/migrations/20261116083957_factory.sql`,
  `20261117093000_factory_check_round.sql`,
  `20261117104713_factory_step_agent.sql`,
  `20261117120425_factory_step_bot.sql`,
  `20261117170000_factory_turn_place.sql`,
  `20261117180000_factory_check_round_verified.sql`,
  `20261117183000_factory_main_hold.sql`,
  `20261117183117_factory_control_files.sql`,
  `20261117184500_factory_review_round.sql`,
  `20261117190000_factory_check_round_commit.sql`,
  `20261117200000_factory_check_round_commit_check.sql`,
  `20261117210000_factory_main_hold_task_key.sql`,
  `20261117215724_factory_step_shared_session.sql`,
  `20261117220000_factory_follow_up_commented.sql`,
  `20261117221000_factory_standing_rules.sql`).
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
- A Step that goes on after its agent's question claims the asking Step's
  session, so a session can have several Steps, one after another: at most
  one of them running (`factory_step_running_session_idx`), and the asking
  Step ended before the next claims it. `session_step` and `hand_back`
  reach the running one. A Gate opened for a Step that ended `question` is
  an agent's question (`GateKind::Question`), read from the Step's outcome.
- A Step's rounds of checks (`factory_check_round`,
  `src/outbound/pg_run_repository/check_rounds.rs`) keep their first
  record, end and fix prompt, so a retried Step goes on with its round.
  `end_check_round_on_quota` keeps a round as ended by the agent's quota
  (`quota_ran_out`): no fix or round follows it. `record_check_result` keeps
  how each check of a round the engine ran itself went, once per check and
  only while the round is open, and marks the round `verified`, keeping
  the commit its first run was on (`checked_commit`); a round the agent
  reported has no `results` and no commit.
  `list_steps` and `list_runs_steps` attach them to their Steps.
- A review round (`factory_review_round`) is keyed by the place of its first
  Step and recorded before its Steps start, so it references the Run, not a
  Step; `start_review_round` keeps its first record. `record_findings`
  replaces a reviewer Step's findings only while the Step has not ended,
  under the Step's row lock, so they are the ones its end goes with.
  `classify_findings`, `link_follow_up`, `follow_up_key`, `reask_action`,
  `fail_review_round` and `end_review_round` keep their first write. A
  round that could not open has no commit and a `failure`; a reviewer's
  answer that was read is in `factory_reviewer_answer`. `follow_up_task_id`
  references `"Document"` and is cleared when the task is deleted.
  `mark_follow_up_commented` sets `follow_up_commented` on a round's
  findings linked to a task, and repeating it changes nothing.
- `factory_run.task_id` references `"Document"` and is cleared when the task
  document is deleted; `link_task` reports a missing task or Run instead of
  failing. `list_task_runs` reads one owner's Runs on a task newest first
  through `factory_run_task_owner_idx` and stops at its limit: keep its
  `WHERE` and `ORDER BY` on that index's columns.
- `answer_gate` runs after the workflow took the answer, so it must not fail on
  an account deleted since: it records the answer with no author instead.
  Who has an account is the authentication service's to say
  (`factory_engine`'s `UserDirectory`); the crate reads `"User"` only for
  whether an account is an agent's.
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
- `factory_run.tampered_files`, `named_control_files` and
  `control_files_unlisted` keep the Control Files the Run's branch last
  changed (`Run::control_files`), replaced by each
  `set_control_file_changes`; both lists null when it changed none.
  `created_in_person`, `control_file_patterns` and `control_files_team_id`
  are set once, by `insert_run` (`NewRun`); the team id is a record, not a
  reference.
  `factory_gate.approved_control_files` keeps the files an answer approved,
  written with the answer by `answer_gate`, and only with one.
- `factory_team_settings` keeps a team's own Control File patterns
  (`team_control_files`, `set_team_control_files`) and Standing Rules
  (`team_rules`, `set_team_rules`); a null column, or no row, means the
  factory's default applies for that setting, each setter leaves the other
  column alone, and a row left with neither is removed. It goes with the
  team. `factory_project_rules` keeps a team's rules for one Project (an
  initiative), and goes with either. Rules are stored as the Gate kinds
  they decide (`plan`, `blockers`, `merge`), checked against those names.
- `factory_run.standing_rules`, `standing_rules_source` and
  `standing_rules_project_id` are set once, by `insert_run`; all null for
  a Run created before Runs kept rules. `answer_gate_by_rule` records a
  Rule Answer (`factory_gate.rule`, with no author, note or approved
  Control Files) and, like `answer_gate`, keeps the first answer. `team_of` reads a
  person's team and role from `team_user`, as other features' records do:
  a person is on at most one team, and an agent's account
  (`"User"."isAgent"`, which `is_agent_account` reads) is never an admin.

- `factory_merge` keeps one row per Run that merged (`record_merge`, safe
  to repeat), with the base it merged into and the time on the worker's
  clock; `recent_merges` reads a base's newest. `factory_main_hold` keeps
  one row per base branch: when its checks last passed (`last_green_at`),
  why its required checks decide nothing (`required_checks`: `unread` or
  `none`, set by `mark_required`, which drops any hold), and, while they
  fail, the hold (`held_since`, `head_sha`, `failed`). All its times are the reads'
  starts on the workers' clock, so they compare with the merges'.
  `hold_main` keeps a hold's start; `clear_main_hold` ends it and its red
  period, unless the hold started at or after the green read did.
  `suspect_merges` reads a base's merges since it was last green through
  `factory_merge_base_idx`, newest first up to its limit, and returns them
  oldest first. A red period files one task: `claim_hold_task` claims it for
  the hold's start once, at the worker's time, or again once a claim never
  filed is older than `TASK_CLAIM_STALE_AFTER`, and returns the id to file
  it with (`task_key`, chosen by the first claim and kept until the hold
  ends, `20261117210000_factory_main_hold_task_key.sql`); `release_hold_task` gives
  a claim back when filing failed, and `record_hold_task` keeps the task's
  id and when it was filed (`task_filed_at`), which a deleted task document
  leaves set, so it is not filed again.

## Related

- Rules: `.agents/rules/rust/db.md` · Docs: `docs/development/database.md`
