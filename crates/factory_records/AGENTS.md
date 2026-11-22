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
- `src/domain/flaws.rs` and `src/outbound/pg_run_repository/flaws.rs`:
  `FlawRepository`, which marks a Run that ended `success` flawed, keeping
  who marked it (its owner, or a Scorer by name), why and when.
- `src/domain/insights.rs` and `src/outbound/pg_run_repository/insights.rs`:
  `RunInsightsRepository`, the reads behind Run insights.
- `src/domain/self_improvement.rs` and
  `src/outbound/pg_run_repository/self_improvement.rs`:
  `SelfImprovementRepository`, a team's failures under its Factory
  Definition and the self-improvement Runs that address them.
- `src/domain/quality.rs` and `src/outbound/pg_run_repository/quality.rs`:
  `RunQualityRepository`, the reads behind CI health, review quality, cost
  and autonomy, and the `ci` Step's waits (`record_ci_wait`).
- `src/domain/main_hold.rs` and `src/outbound/pg_run_repository/main_hold.rs`:
  `MainHoldRepository`, the merges Runs made and the Main Holds on the base
  branches they merge to.
- `src/domain/merge_guard.rs` and
  `src/outbound/pg_run_repository/merge_guard.rs`: `MergeGuardRepository`,
  the Run that has not ended and opened a pull request, whoever owns it:
  Macro's GitHub merge route refuses to merge under it.
- `src/domain/standing_rules.rs` and
  `src/outbound/pg_run_repository/standing_rules.rs`:
  `StandingRulesRepository`, teams' and Projects' Standing Rules, and the
  Gate kinds, rules and Orchestrator Definition they are made of.
- `src/domain/standing_acts.rs` and
  `src/outbound/pg_run_repository/standing_acts.rs`: `StandingActRepository`,
  the bot behind an agent session of a Run's owner (`agent_session.bot_id`)
  and what agents did to Runs under a Factory Definition's Standing Roles
  (`factory_standing_act`): one start, one cancel and one answer per Gate,
  every link, each with its bot and its agent session, if it came from one.
- `src/domain/automations.rs` and
  `src/outbound/pg_run_repository/automations.rs`: `AutomationRepository`,
  teams' Automations and each time one fired.
- `src/domain/board.rs` and `src/outbound/pg_run_repository/board.rs`:
  `RunBoardRepository`, the board's read of Runs across tasks, and of the
  Runs that opened some pull requests.
- `src/domain/evaluation.rs` and
  `src/outbound/pg_run_repository/evaluation.rs`: `EvaluationRepository`,
  Experiment arms, the Agent Rules that chose a Run's Agents, Scorers'
  judgments, Benchmark suites and trials.
- `src/domain/merge_queue.rs` and
  `src/outbound/pg_run_repository/merge_queue.rs`: `MergeQueueRepository`,
  each base branch's merge queue and merge turn, the updates of Runs'
  branches, and the head each Run's review last passed on.
- `src/domain/gate_replies.rs` and
  `src/outbound/pg_run_repository/gate_replies.rs`: `GateReplyRepository`,
  a person's answer read from their words, the factory asking back, the
  stand-in's answer or why it left the Gate to the owner (the Gate model's
  reading, as for asking back), the owner's past answers and the answered
  Gates a replay measures the model on.

## Scope

- Owns: the `factory_run`, `factory_step`, `factory_gate`,
  `factory_check_round`, `factory_team_settings`, `factory_merge`,
  `factory_main_hold`, `factory_review_round`, `factory_review_finding`,
  `factory_project_rules`, `factory_automation`,
  `factory_automation_firing`, `factory_team_definition` and
  `factory_standing_act` tables
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
  `20261117221000_factory_standing_rules.sql`,
  `20261118004853_factory_gate_replies.sql`,
  `20261118005054_factory_owner_gates.sql`,
  `20261118120000_factory_owner_gates_and_stand_in.sql`,
  `20261118130000_factory_task_thread.sql`,
  `20261118131500_factory_queue_limit.sql`,
  `20261118141500_factory_orchestrator_answers.sql`,
  `20261118143000_factory_automation.sql`,
  `20261118150000_factory_standing_rules_names.sql`,
  `20261118160000_factory_queue_limit_minimum.sql`,
  `20261118170000_factory_step_terminal.sql`,
  `20261118180000_factory_step_pause_limit.sql`,
  `20261118190000_factory_run_board.sql`,
  `20261118200000_factory_gate_model_readings.sql`,
  `20261118200200_factory_gate_asked_back_tampered.sql`,
  `20261118210000_factory_evaluation.sql`,
  `20261118210100_factory_benchmark_trial_base.sql`,
  `20261118210200_factory_benchmark_suite_planned_trials.sql`,
  `20261118213000_factory_definition.sql`,
  `20261118213100_factory_run_flow.sql`,
  `20261118213200_factory_run_definition_evaluation.sql`,
  `20261118220000_factory_run_outcome_kinds.sql`,
  `20261118233000_factory_run_pull_request_url_index.sql`,
  `20261119000000_factory_standing_act.sql`,
  `20261119000100_factory_standing_act_sessionless.sql`,
  `20261119003000_factory_repository_setup.sql`,
  `20261119010000_factory_quiet_gates.sql`,
  `20261119020000_factory_self_improvement.sql`,
  `20261119020100_factory_improvement_failure_dismissed.sql`,
  `20261119030000_factory_run_cancelled_in_person.sql`,
  `20261119040000_factory_merge_queue.sql`,
  `20261119043000_factory_ci_wait.sql`,
  `20261119044000_factory_ci_job_failure.sql`,
  `20261119045000_factory_ci_job_replaced.sql`,
  `20261119050000_factory_token_budget.sql`,
  `20261119050100_factory_token_budget_validate.sql`,
  `20261119080000_factory_step_used_session.sql`,
  `20261120010000_factory_improvement_dismissal_undone.sql`,
  `20261120010100_factory_improvement_dismissal_undone_validate.sql`,
  `20261120020000_factory_run_agent_rule.sql`,
  `20261120020100_factory_run_task_tags_unread.sql`,
  `20261120030000_factory_review_dispute.sql`,
  `20261120030100_factory_review_dispute_validate.sql`), `factory_step_steer`,
  `factory_run_score`, `factory_run_arm`, `factory_run_agent_rule`,
  `factory_run_task_tags_unread`,
  `factory_benchmark_suite`,
  `factory_benchmark_trial`, `factory_benchmark_trial_score`,
  `factory_improvement`, `factory_improvement_failure`, `factory_merge_queue`,
  `factory_branch_update`, `factory_reviewed_head`, `factory_ci_wait`,
  `factory_ci_job` and `factory_step_used_session`.
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
- The Step Terminal (`src/outbound/pg_run_repository/steering.rs`): `steer`
  keeps what the owner did on a running Step (`factory_step_steer`, in
  order) and the steering its work reads (`paused_at`, `stop_asked_at`,
  `steer_action_id`, the prompt it follows), under the Step's row lock,
  only while the Step and its Run have not ended, it holds its session and
  its steering is open (`steer_closed`). `close_steering` closes it only
  while the Step follows the prompt the work names and is neither paused
  nor asked to stop; `begin_turn` opens it again and drops the prompt it
  followed. `unsteer` marks a refused prompt undelivered and goes back to
  the one before. A note keeps the hand back. `list_steps` reads a running
  Step's session (`terminal.session_id`), none once it ended or let go of it.
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
  `record_fix` keeps what the fix said of each Blocker of a round
  (`fix_status`, and the `dispute` reason, at most 2 KiB), replacing it
  while no reviewer has ruled on it. `record_answer` is `record_findings`
  with the reviewer's rulings on the disputes of the round before
  (`ruling`, `ruled_by`): they replace what an earlier attempt of its Step
  ruled, in the same transaction, and a dispute another Step ruled on
  keeps its ruling. A finding that repeats a Blocker its reviewer dropped
  (`repeats_dropped`) is classified `dismissed` when it is in the diff under
  review. Insights' Blocker records carry dismissed findings too
  (`dismissed`), which count only as dismissed. A binary older than these
  columns fails to read a `dismissed` kind (`UnknownReviewWord`): deploy
  the migration, then every reader at once.
- `list_waiting_runs` reads one owner's Runs that have not ended and wait
  on an unanswered Gate, newest first, through `factory_run_user_open_idx`
  (open Runs only) and the Gates' primary key, up to its limit.
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
- `factory_run.project_id` is the Project the Run's task was in when it was
  kept, set once by `insert_run` (`NewRun::project_id`): a record, not a
  reference, null for Runs kept before it. `pull_request_url` is the page of
  the Run's pull request, replaced by `set_pull_request_url` each time a
  pull request Step finds it.
- `board_runs` reads, for each owner (the person, and for a team's board
  `team_user`'s members of `team_id`), their newest Runs past a
  `(created_at, id)` cursor in order through `factory_run_user_created_idx`,
  up to the limit, then merges the owners' Runs newest first and cuts them
  to it; the states of Runs that go read through `factory_run_user_open_idx`
  in a query of their own with a literal `ended_at IS NULL`. A first page
  starts past the latest time there is. It runs under a 5 s timeout and
  `plan_cache_mode = force_custom_plan`: a generic plan, which a reused
  statement may get, cannot tell which filters are unset and reads and
  sorts every Run of the owners (on 280k Runs, 90 to 330 ms against 0.2 to
  0.5 ms). Where a Run is (`BoardState`) is read in the same query: its
  outcome, an unanswered Gate, and for a Run whose latest Step is at its
  merge (an entry that runs `ready_to_merge` or `merge` in the Flow the Run
  kept, in its own row; for a Run that kept none, one of the built-in merge
  entries given), its base's Main Hold, each by primary key; it also
  reads whether its task is a live task (`task_live`: a task document, not
  trashed) and, only then, its name. A search reads the request and a live
  task's name with `ILIKE`, its `%`, `_` and `\` literal, and so reads
  every Run of the owners (some 200 ms on 280k). A held Run is held only at
  its merge, so a merged Run is `finished`, as is a `flawed` one. A
  Benchmark's trials (Flow `benchmark`) are no one's work: neither of its
  queries reads them.
- `pull_request_runs` reads, for each key asked (`pull_request_key`) on its
  own, the Runs whose `lower(pull_request_url)` is that key, through
  `factory_run_pull_request_url_idx` (one index search per key), of the
  person or a member of the team given: only the newest Run of each owner
  and task (`DISTINCT ON`, since whether a caller sees a Run depends only on
  those), newest first up to the limit for that key, so one pull request's
  Runs never crowd out another's. Each is where it is as `board_runs` reads
  it (a flawed Run finished, one at its merge by the Flow it kept); the rows
  come newest first across keys. A Benchmark's trials are read: a trial
  opens no pull request, and a page kept on one by hand still guards its
  merge.
- `read_insights` reads one person's newest Runs in a period through
  `factory_run_user_created_idx` (`user_id, created_at DESC, id DESC`), up to
  its cap (one past it, to tell whether Runs were left out), then their Steps
  and Gates by primary key, in one read-only
  snapshot under a 5 s timeout. Keep its first `WHERE` and `ORDER BY` on that
  index, and every later read on the Runs' ids.
- `insert_suite` counts one person's starts one after the other (an
  advisory lock on their id in its transaction) and refuses a suite while
  one of the same Benchmark of theirs runs (a trial's Run not ended, or no
  trial yet and started in the last five minutes) or its trials would pass
  `MAX_OPEN_TRIALS`. `read_evaluation` lists only suites with a trial.
- A trial's judgments (`factory_benchmark_trial_score`) never go in
  `factory_run_score`, and `read_insights` and `read_evaluation` leave Runs
  of the `benchmark` Flow out, so a Benchmark moves no live number.
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
  they decide (`plan`, `blockers`, `merge`), `owner_gates` when the
  definition's owner section is on, `quiet_gates` when its Gates notify
  no one, the kinds the Orchestrator may approve
  (`orchestrator_plan`, `orchestrator_blockers`), and `task_thread` when its
  task thread section is on (`OrchestratorDefinition::kinds`), checked
  against those names (`20261119010000_factory_quiet_gates.sql`
  allows every one of them). A definition kept before a section has none of
  it, and reads with it off. `quiet_gates` is an opt-out, so a definition
  without it notifies: the migration that added it set it on no row, so
  every kept definition (a team's, a Project's and a Run's), its owner
  section off or on, notifies until an admin turns it off. The definition's Queue limit has a
  column of its own beside them (`queue_limit`,
  `20261118131500_factory_queue_limit.sql`, held from `MIN_QUEUE_LIMIT`
  to `MAX_QUEUE_LIMIT`, 1 to 1000, by
  `20261118160000_factory_queue_limit_minimum.sql`), null
  for no limit, and none on a team's row without Standing Rules; a Run
  does not keep it, so a Run's rules read with none. The definition's
  token budget has a column of its own too (`token_budget`,
  `20261119050000_factory_token_budget.sql`, from `MIN_TOKEN_BUDGET` to
  `MAX_TOKEN_BUDGET`, 10,000 to 10,000,000,000, null for none, and none on
  a team's row without Standing Rules), and, unlike the Queue limit, a Run
  keeps it: `insert_run` writes the rules' budget to
  `factory_run.token_budget`, which the Run's rules read back, so a change
  to the definition later does not change it. `record_budget_stop` keeps
  why the budget ended a Run (`token_budget_stop`: `spent` with
  `token_budget_used`, or `unread`), only on a Run with a budget and only
  the first time (`BudgetStop` on the Run). `step_sessions` reads the
  agent sessions a Run's Steps worked in, each once, as Insights does.
  `factory_step.session_id` is only a Step's current session, which
  `pass_session` replaces when the Step goes on in a new one (its session
  was lost, or its Agent could not start); `factory_step_used_session`
  keeps every session a Step worked in, written in the same statement as
  `claim_session` and `pass_session` (both the session left and the new
  one), so a session lost after its agent worked in it still counts.
  `20261119080000_factory_step_used_session.sql` backfilled each Step's
  current session; the sessions Steps left before it were never kept. The
  reads (`step_sessions`, Insights' `read_insights` and `read_quality`,
  and `scoring_subject`'s `sessions` for a Benchmark trial's tokens) take
  the union of both, so a session counts once however many Steps or rows
  keep it, and a Step claimed by a build older than the table still
  counts its current session. `read_insights` returns one
  `StepSessionRef` per Step and session, with the Step's `seq`, so
  Insights count Steps apart from sessions. While a build older than the
  table still runs after the migration (a rolling deploy), its
  `pass_session` keeps no row, so the session that pass leaves is
  dropped from the count; the next pass by a newer build keeps the
  session it leaves. The
  kinds the
  Orchestrator may approve are kept in the same arrays as the rules
  (`orchestrator_plan`, `orchestrator_blockers`), and on the Run.
- `answer_gate` keeps the agent session that gave an answer for the owner
  (`factory_gate.agent_session_id`, only with a person's answer, never a
  rule's or the stand-in's); `past_answers`, `answered_gates` and Run
  insights leave such answers out, as they do the stand-in's.
  `insert_run_in_queue` keeps every new Run behind a transaction-scoped
  advisory lock on its owner (`factory_queue:<user id>`, taken under a 5 s
  `lock_timeout`); under a Queue limit it first counts the places the owner
  holds (`queue_places`: their open Runs at an unanswered Gate the stand-in
  has not answered, and those of the given `asking_flows` that have not
  reached a Gate yet, through `factory_run_user_open_idx`, up to the
  limit) and keeps nothing once they hold as many as the limit
  (`RunInsert::QueueFull`, with the counts), so a limit of N lets them
  hold at most N. `insert_run` takes the same lock with no limit.
  `list_starting_runs` lists the second kind.
- `factory_run.standing_rules`, `standing_rules_source` and
  `standing_rules_project_id` are set once, by `insert_run`; all null for
  a Run created before Runs kept rules. `answer_gate_by_rule` records a
  Rule Answer (`factory_gate.rule`, with no author, note or approved
  Control Files) and, like `answer_gate`, keeps the first answer. `team_of` reads a
  person's team and role from `team_user`, as other features' records do:
  a person is on at most one team, and an agent's account
  (`"User"."isAgent"`, which `is_agent_account` reads) is never an admin.

- Gate replies (`20261118004853_factory_gate_replies.sql`):
  `answer_gate_from_reply` records a person's answer read from their
  words, which it keeps as the note, with how sure the factory was
  (`reply_confidence`, in percent; none for a question Gate, whose answer
  is the words); `ask_back` keeps the latest reply the factory could not
  read (`asked_back_reply`, `asked_back_at`) while the Gate has no answer.
  `stand_in` keeps the stand-in's answer, reason, confidence and override
  window (`stand_in_*`) once, on a Gate with no answer;
  `answer_gate_by_stand_in` makes it the Gate's answer (`by_stand_in`, no
  author, note or approved Control Files) only while the Gate still has
  none, so an owner's override in the window stands. Every write keeps the
  Gate's first answer. The stand-in's kinds are kept in the same
  `standing_rules` arrays as the rules and the owner section
  (`stand_in_plan`, `stand_in_blockers`; it has none for a merge Gate);
  `20261118120000_factory_owner_gates_and_stand_in.sql` checks those
  arrays against every name of both. `withdraw_stand_in`
  takes the stand-in's answer back while the Gate has no answer, when the
  Run no longer lets it stand. `past_answers` reads the owner's own
  answers to an entry's Gates through their newest Runs
  (`factory_run_user_created_idx`), in Runs of the same Flow only, never
  an agent's question asked at the entry, nor the Gate of an agent that
  stopped without a result (a Step that ended `no_result`); `answered_gates`
  reads every Gate a person answered with a button (no answer read from
  words, nor the stand-in's, nor a `no_result` Step's), with the Run's plan, for the offline replay measure only.
  Run insights leave out the stand-in's answers, as they do Rule
  Answers.

- Automations (`20261118143000_factory_automation.sql`): a
  `factory_automation` belongs to its team and goes with it, and with its
  creator's account; its trigger is kept as JSON beside its kind, and a
  webhook's secret only for a webhook (`factory_automation_webhook`).
  `claim_firing` claims a firing's key once, at the database's time, unless
  one that never ended is older than `FIRING_CLAIM_STALE_AFTER`: then it
  first looks for a Run started under the key, and ends the firing with it;
  `end_firing` keeps the first end. `factory_run.automation_id`,
  `automation_name`, `automation_trigger` and `automation_firing_key` are
  set once, by `insert_run` (`NewRun::automation`), and are a record, not a
  reference: they outlive the Automation. `factory_run_automation_firing_idx`
  refuses a second Run of one firing (`RunInsert::FiringStarted`). `factory_run_automation_task_open_idx` refuses a second
  open Run an Automation started on a task, which `insert_run` answers as
  `RunInsert::TaskBusy`; `running_run_on_task` reads any open Run on a task
  through `factory_run_task_owner_idx`.

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

- Self-improvement (`20261119020000_factory_self_improvement.sql`):
  `factory_team_definition.named_by_user_id` keeps who named a team's
  Definition (`set_team_definition`; null for one named before, or once
  their account is gone). In the same transaction, `set_team_definition`
  settles the team's open improvement on another repository (ignoring
  case), or any when it names none, as `closed`. `record_failures` keeps a Run's failure once per
  Run and source (`kind` `scorer` with its name and score, or `flawed`
  with `scorer` ''), with its team and the Definition repository it ran
  under. A failure is pending while `improvement_id` is null;
  `pending_failures` counts them through
  `factory_improvement_failure_pending_idx` (team, oldest first), matching
  the Definition's repository ignoring case, as `begin_improvement`'s claim
  does.
  `begin_improvement` keeps a self-improvement Run as `starting` and
  claims the oldest pending failures in one transaction;
  `factory_improvement_open_idx` holds a team to one `starting` or `open`
  Run, so a second begin keeps nothing (`AlreadyOpen`), and one with
  nothing pending keeps nothing. `link_improvement_run` opens it with its
  Run, only while starting. `settle_improvement` settles an open one once:
  `merged` marks its failures addressed (`addressed_at`), a person's
  rejection (`Settlement::Rejected`, kept `closed`; a cancel is one only
  when `failure_facts` reads `cancelled_in_person`, which
  `record_cancel_sender` sets or clears for each cancel while the Run has
  not ended, `20261119030000_factory_run_cancelled_in_person.sql`) dismisses them
  (`dismissed_at`, `20261119020100_factory_improvement_failure_dismissed.sql`;
  they keep its `improvement_id`, so they are never pending again), and
  `closed` and `refused` (with why) clear their `improvement_id`, so they
  are pending again. `undo_dismissal` undoes a rejection's dismissal once,
  in one transaction with the improvement row locked: the failures it
  still holds dismissed (`improvement_id` and `dismissed_at` set) are
  pending again, and the improvement keeps who and when
  (`dismissal_undone_by_user_id`, `dismissal_undone_at`,
  `20261120010000_factory_improvement_dismissal_undone.sql`); a second undo
  answers the kept one and changes nothing, so failures a later improvement
  took stay with it, and one that dismissed nothing is
  `NothingDismissed`. Reads of an improvement count the failures it holds
  dismissed (`dismissed`, `factory_improvement_failure_improvement_idx`);
  `improvement` reads one of a team's by id. `failures` keeps how many it claimed. `recent_improvements` reads
  a team's latest ones, newest first, for the pause after those that did
  not merge (`factory_improvement_team_created_idx`). Neither table decides
  who merges: the `improve` Flow keeps its plan and merge Gates for a
  person (`crates/factory_engine`). Both tables go with the
  team; a failure goes with its Run, and an improvement's Run is cleared
  when the Run goes.

- `factory_merge_queue` keeps one row per Run that reached its merge
  (`enter_merge_queue`, safe to repeat, keeps `entered_at` and `place`, and
  clears `left_at`), with its base: the Run's repository URL lower-cased,
  trimmed and without a `.git` ending, and its branch; it goes with its
  Run. `factory_main_hold` still keys a base by the URL as the Run kept it,
  so the same repository named in another case is held apart.
  `merge_queue_ahead` reads, from the Run's own row, the Runs of the same
  base before it (by `place`, a serial taken as each entered, so Runs that
  entered in the same instant keep their order, through
  `factory_merge_queue_base_idx`) that have not ended and are at their
  merge or left it within the place kept, all on the database's clock.
  `leave_merge_queue` keeps the first time it left; `drop_from_merge_queue`
  takes a merged Run out, and `end_run` one that ended unmerged.
  `take_merge_turn` takes a base's merge turn: it reads the Run's
  repository and base, outside the turn, as the base's key, then holds a
  transaction that sets its own `idle_in_transaction_session_timeout`
  (`set_config(..., true)`) and runs `SELECT pg_try_advisory_xact_lock($key)`
  (`None` while another Run holds it; never waited on, so no connection
  blocks). The transaction reads, locks and writes no factory table, so a
  turn held for minutes holds back no write to a Run nor a migration of
  its table; the key is the one earlier workers took, so a rolling deploy
  keeps one turn per base (`a_merge_turn_keeps_the_key_earlier_workers_took`
  pins it).
  The turn is the transaction: dropping it rolls back, which lets the
  lock go once the pool takes the connection back, on every path, and the
  transaction's `idle_in_transaction_session_timeout` (five minutes) ends
  one a stopped worker left open. It holds a pooled connection while a
  merge runs, one per base merging. `factory_branch_update` keeps an update Step's
  first update (`keep_branch_update`, by `(run_id, seq)`), replaced only
  while GitHub's merge of it never showed (`restart_branch_update`: no
  conflict, no head recorded), and
  `set_updated_head` records the head GitHub's merge made once, only for an
  update with no conflict. `factory_reviewed_head` keeps one head per Run
  (`set_reviewed_head` replaces it, `reviewed_head` reads it): written only
  when a review round passes and when an update merged only the base into
  it, never by `restart_branch_update`, so an update asked again from a
  pushed head leaves it as the review left it.

## Related

- Rules: `.agents/rules/rust/db.md` · Docs: `docs/development/database.md`
