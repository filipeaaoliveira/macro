# factory_engine

## What it is

The software factory's engine: runs a Flow, a JSON list of entries whose Step
outcomes say where a Run goes next, as one durable Temporal workflow per Run.

## How it works

`POST /runs` parses the Flow, records the Run (`factory_records`), and starts
`FlowWorkflow` with the Run's id as its workflow id. The workflow asks the
interpreter for the next Step, runs it as the `factory_run_step` activity,
feeds the outcome back, and records the Run's end with `factory_end_run`.

## Start here

- `src/domain/flow.rs`: the Flow model and the checks a definition must pass.
- `src/domain/interpreter.rs`: `when`, `max_visits`, `on_exhausted`, `on`,
  `next`, facts and resets, with no Temporal in it.
- `src/inbound/temporal/workflow.rs`: the workflow that drives the interpreter.
- `src/temporal.rs`: the task queue and the workflow's name and types.
- `flows/`: example Flows, read by the tests.

## Scope

- Owns: the Flow format, its interpreter, the workflow and its activities, the
  Run API's router, and the built-in Steps (`reply`).
- Not here: the Run and Step tables → `crates/factory_records`; the process
  that hosts the API and the worker → `services/factory_service`.

## Boundaries

- Depends on: `factory_records`, `macro_authorization`, the Temporal Rust SDK.
- Used by: `services/factory_service`.

## Recipes

- Add a built-in Step: `src/outbound/built_in_steps.rs` (`serves` and `run`).
- Add a Flow keyword: `src/domain/flow.rs` (field and check) →
  `src/domain/interpreter.rs` → `src/domain/interpreter/test.rs`.

## Checks

- `cargo test -p factory_engine`

## Gotchas

- The workflow must stay deterministic: everything it decides comes from the
  interpreter and the activities' results, never a clock, a random number or I/O.
- Renaming `TASK_QUEUE`, `FLOW_WORKFLOW` or an activity's `name` strands the Runs
  in flight on the old name.
- `#[activities]` takes no generic type, so `FactoryActivities` holds the Step
  service as a trait object.
