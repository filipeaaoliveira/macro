# factory_engine

## What it is

The software factory's engine: runs a Flow, a JSON list of entries whose Step
outcomes say where a Run goes next, as one durable Temporal workflow per Run.

## How it works

`POST /runs` reads its Flow, records the Run (`factory_records`), and starts
`FlowWorkflow` with the Run's id as its workflow id. For each Step the
interpreter names, the workflow records its start (`factory_start_step`), runs
its work (`factory_run_step`), records its end (`factory_end_step`), and feeds
the outcome back; then it records the Run's end (`factory_end_run`). A Step is
keyed by its place among the Run's Steps (`seq`), which a reset never reuses.
Work that fails for good, or still fails after two hours of retries (not a count
of attempts, which a worker shutting down uses up), ends the Step `failed`, with
a fixed output (why is in the worker's logs). The work heartbeats every 2 s
against a 30 s timeout, and the worker holds a heartbeat back at most 4 s, so a
cancel reaches it within about 4 s. A cancelled activity only drops its work: it
can't tell the Run's cancellation from a worker shutting down or a timed-out
attempt, and a retry goes on in the same agent session. Once the Step's or the
Run's end is recorded, the workflow stops the agent (`factory_stop_step`, on a
token the cancellation does not reach) when the Run was cancelled, or the work
failed for good: the stop is sent again for a minute while the session still
works (a prompt on its way can restart it), then the session is deleted. A
failed delete is retried, unless no attempt saw the session open and its last
prepare (recorded at the database's time before each) is over five minutes old:
then the session is not there. A delete that answers a missing session as
deleted is retried too while such a prepare may still open it. A Step that ended
has its session deleted (`factory_release_step`). Either way the Step first
records that it let go of the session. An attempt checks that record before it
prepares and before it prompts, so none starts either after it; one that passed
a check just before can still prepare or prompt, and the stop's watch and
retried delete cover it. A prepare refused outright, or never shown open, is no
longer in flight. A shutting-down worker gives running Steps a few seconds, then
cancels them, so Temporal retries them on another worker at once. A cancelled
Run's open Steps end `cancelled` with it. A Run takes at most `MAX_STEPS` Steps,
then ends `blocked`. Workflow inputs carry ids, never the request's text.

An entry with a `gate` asks a person a question once its Step ends `done`: the
workflow records the Gate (`factory_open_gate`), waits for the
`factory_answer_gate` update that `POST /runs/{id}/steps/{seq}/answer` sends,
records the answer (`factory_answer_gate` activity), and routes the answer as
the entry's outcome. An answer the recorded Gate does not offer is refused
before it is sent (400); the update's validator (`domain/gate.rs`) refuses an
answer to a Gate the Run does not wait on and a second answer (409). Only the
Run's owner answers its Gates, and only as a person: an agent's program acting
for them (`is_agent_run`) is refused (403).

People call the Run API with their own login (`UserOrInternal`), and internal
services for an acting user (the internal key's identity headers). A Run starts
only of a built-in Flow, by name: no caller sends a Flow definition. The acting
user owns the Runs they create, and reads, answers, links and cancels only
those; someone else's Run reads as missing (404). A call with no user is
refused (401), and one whose user has no account (400). A Run may work on a
Macro task: `POST /runs` takes an optional `taskId` (the task document's id),
and `PUT /runs/{id}/task` links or relinks one. The owner must see the document
through entity access, and document storage must say it is a task that is not
deleted (`outbound/document_task_directory.rs`); a hidden document and a missing
one are both "no task" (400). `GET /tasks/{task_id}/runs` lists the caller's own
Runs on a task they can see, newest first, at most `TASK_RUNS_LISTED`; other
people's Runs on it are not listed, and a hidden or missing task is 404.

`POST /runs/{id}/cancel` cancels the workflow through Temporal; cancelling a
cancelled Run again is accepted, and a Run whose workflow never started ends
`cancelled` (its workflow, should it start late, takes no Step). Reading a Run
whose workflow never started ends it `failed` once it is five minutes old. A
Temporal outage answers 503 (or 502 for an unusable answer), never 500.

The built-in Steps (`src/domain/built_in_steps.rs`) are `reply`, which
acknowledges the request, and `plan` and `write`, which hand an agent a prompt
built from the Step's instructions (`built_in_steps/*.md`), the Run's request,
and what the Run's earlier Steps produced. The agent is the `StepAgent` port.
`SessionAgent` (`src/domain/session_agent.rs`) does the Step in a Macro agent
session of the configured agent, owned by the Run's owner, through the routine
sessions capability (`agent_harness_service_client`). The Step claims its
session in `factory_step` first, so a retried Step goes on in the same session
and does not start the work over: it always asks, and the session refuses a
prompt it already has with a conflict. That guard is a check of the session's
history, not exactly-once delivery, so a prompt whose delivery is unknown is
never sent again: the session's status settles it, and one that never shows
fails the Step for good. The prompt asks the agent to hand back its result with
`macro.factory.handBack({ outcome, text })` from code mode, which reaches `POST
/hand-backs` as the Run's owner; the correlation layer names the session
(`agent_correlation::context::current_session`), and the hand back must offer
one of the Step's outcomes. When the session's first turn ends, the latest hand
back is the Step's outcome and output; none fails the Step. `LocalAgent` does no
work and hands back the prompt it was given: the tests' stand-in, and the
service's when no agent is configured.

## Start here

- `src/domain/flow.rs`: the Flow model and the checks a definition must pass.
- `src/domain/interpreter.rs`: `when`, `max_visits`, `on_exhausted`, `on`,
  `next`, facts and resets, with no Temporal in it.
- `src/inbound/temporal/workflow.rs`: the workflow that drives the interpreter.
- `src/temporal.rs`: the task queue and the workflow's and its update's names
  and types.
- `src/domain/gate.rs`: the Gate a Run waits on and the answers it takes.
- `src/domain/built_in_steps.rs`: the Steps the worker serves and their
  prompts.
- `src/domain/session_agent.rs`: an agent Step in a Macro agent session.
- `flows/`: the built-in Flows, each named for its file
  (`src/domain/built_in_flows.rs`). `GET /flows` lists those whose Steps the
  worker serves, with their `description`: `plan` and `reply`; `code.json`
  also names Steps no worker serves yet.

## Scope

- Owns: the Flow format, its interpreter, Gates, the workflow and its
  activities, the Run API's router, the built-in Flows (`flows/`) and Steps
  (`reply`, `plan`, `write`), and the `StepAgent` port with its local
  stand-in.
- Not here: the Run, Step and Gate tables → `crates/factory_records`; the process
  that hosts the API and the worker → `services/factory_service`.

## Boundaries

- Depends on: `factory_records`, `agent_session` (the routine sessions port),
  `agent_correlation`, `macro_authorization`, `macro_user_id`,
  `entity_access` (ports), `document_storage_service_client`, the Temporal
  Rust SDK.
- Used by: `services/factory_service`.

## Recipes

- Add a built-in Flow: a file in `flows/` with a `description`, and an entry
  in `BUILT_IN_FLOWS` (`src/domain/built_in_flows.rs`).
- Add a built-in Step: `src/domain/built_in_steps.rs` (`BuiltInStep`, `named`
  and `run`); an agent Step also gets its instructions in `built_in_steps/`.
- Add a Flow keyword: `src/domain/flow.rs` (field and check) →
  `src/domain/interpreter.rs` → `src/domain/interpreter/test.rs`.

## Checks

- `cargo test -p factory_engine`
- The workflow on a running Temporal (`src/inbound/temporal/test.rs`, ignored
  in CI): `cargo test -p factory_engine -- --ignored` with the local stack
  up, or `TEMPORAL_ADDRESS` set to another dev server.

## Gotchas

- A Step or Run that ended can still have its agent at work until the stop
  lands, up to the stop's fifteen minutes of retries. Its hand back is refused
  meanwhile: the Step's end is recorded first, and a hand back needs a Step
  that has not ended. If every stop fails, nothing in the factory ends the
  agent's turn: it runs until the agent ends it, in a session no attempt
  opens or prompts again.
- A runtime that opens a requested session more than five minutes after its
  prepare, as macrod may when its event stream lags or replays, leaves a
  session no stop deletes: by then the stop has written it off. The recorded
  prepare and open narrow that window; no bound closes it.
- `RoutineSessionError::is_transient` (in `agent_session`) exists for the
  factory's retries; it is not meant for upstream on its own.
- The workflow must stay deterministic: everything it decides comes from the
  interpreter and the activities' results, never a clock, a random number or I/O.
- Renaming `TASK_QUEUE`, `FLOW_WORKFLOW`, `ANSWER_GATE_UPDATE` or an
  activity's `name` strands the Runs in flight on the old name.
- Changing what the workflow or the interpreter decides breaks the replay of
  Runs already running: gate the change with Temporal patching
  (`ctx.patched`) or worker versioning.
- A Gate opens only for a Step that ends `done`; any other outcome routes as
  usual, so a failed plan is never put to a person.
- `#[activities]` takes no generic type, so `FactoryActivities` holds the Step
  service as a trait object.
