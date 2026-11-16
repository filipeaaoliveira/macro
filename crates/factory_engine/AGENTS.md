# factory_engine

## What it is

The software factory's engine: runs a Flow, a JSON list of entries whose Step
outcomes say where a Run goes next, as one durable Temporal workflow per Run.

## How it works

`POST /runs` parses the Flow, records the Run (`factory_records`), and starts
`FlowWorkflow` with the Run's id as its workflow id. The workflow asks the
interpreter for the next Step, runs it as the `factory_run_step` activity,
feeds the outcome back, and records the Run's end with `factory_end_run`.

An entry with a `gate` asks a person a question once its Step ends `done`:
the workflow records the Gate (`factory_open_gate`), waits for the
`factory_answer_gate` update that `POST /runs/{id}/gates/{entry}/{visit}/answer`
sends, records the answer (`factory_answer_gate` activity), and routes the
answer as the entry's outcome. The update's validator (`domain/gate.rs`)
refuses an answer to a Gate the Run does not wait on, a second answer, and an
answer the Gate does not offer; the API answers those 409.

Every Run has an owner: `POST /runs` takes the starting user's `userId`, which
must name an account (400 otherwise), and the Run's agent Steps run as them. A
Run may work on a Macro task: `POST /runs` takes an optional `taskId` (the task
document's id), and `PUT /runs/{id}/task` links or relinks one. The owner must
see the document through entity access, and document storage must say it is a
task (`outbound/document_task_directory.rs`); a hidden document and a missing
one are both "no task" (400). A Gate answer from a user id with no account is
refused (400) before it reaches the workflow.

`POST /runs/{id}/cancel` cancels the workflow through Temporal. The running
Step or the wait for an answer stops, and the workflow records the Run's end
as `cancelled` on a cancellation token of its own.

The built-in Steps (`src/domain/built_in_steps.rs`) are `reply`, which
acknowledges the request, and `plan` and `write`, which hand an agent a prompt
built from the Step's instructions (`built_in_steps/*.md`), the Run's request,
and what the Run's earlier Steps produced. The agent is the `StepAgent` port.
`SessionAgent` (`src/domain/session_agent.rs`) does the Step in a Macro agent
session of the configured agent, owned by the Run's owner, through the routine
sessions capability (`agent_harness_service_client`). The Step claims its
session in `factory_step` first, so a retried Step goes on in the same session
and never prompts twice. The prompt asks the agent to hand back its result
with `macro.factory.handBack({ outcome, text })` from code mode, which reaches
`POST /hand-backs` as the Run's owner; the correlation layer names the session
(`agent_correlation::context::current_session`), and the hand back must offer
one of the Step's outcomes. When the session's first turn ends, the latest
hand back is the Step's outcome and output; none fails the Step. `LocalAgent`
does no work and hands back the prompt it was given: the tests' stand-in, and
the service's when no agent is configured.

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
- `flows/`: example Flows, read by the tests. `plan.json` runs on the
  built-in Steps; `code.json` also names Steps no worker serves yet.

## Scope

- Owns: the Flow format, its interpreter, Gates, the workflow and its
  activities, the Run API's router, the built-in Steps (`reply`, `plan`,
  `write`), and the `StepAgent` port with its local stand-in.
- Not here: the Run, Step and Gate tables → `crates/factory_records`; the process
  that hosts the API and the worker → `services/factory_service`.

## Boundaries

- Depends on: `factory_records`, `agent_session` (the routine sessions port),
  `agent_correlation`, `macro_authorization`, `macro_user_id`,
  `entity_access` (ports), `document_storage_service_client`, the Temporal
  Rust SDK.
- Used by: `services/factory_service`.

## Recipes

- Add a built-in Step: `src/domain/built_in_steps.rs` (`BuiltInStep`, `named`
  and `run`); an agent Step also gets its instructions in `built_in_steps/`.
- Add a Flow keyword: `src/domain/flow.rs` (field and check) →
  `src/domain/interpreter.rs` → `src/domain/interpreter/test.rs`.

## Checks

- `cargo test -p factory_engine`

## Gotchas

- The workflow must stay deterministic: everything it decides comes from the
  interpreter and the activities' results, never a clock, a random number or I/O.
- Renaming `TASK_QUEUE`, `FLOW_WORKFLOW`, `ANSWER_GATE_UPDATE` or an
  activity's `name` strands the Runs in flight on the old name.
- A Gate opens only for a Step that ends `done`; any other outcome routes as
  usual, so a failed plan is never put to a person.
- `#[activities]` takes no generic type, so `FactoryActivities` holds the Step
  service as a trait object.
