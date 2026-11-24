# agent_harness_service_client

## What it is

Part of agent sessions: the HTTP client other services use to run agent
sessions through `agent_harness_service`'s internal routes. It implements the
`RoutineSessions` port of `crates/agent_session` over
`/internal/routine-sessions/*`.

## Start here

- `src/routine_sessions.rs`: `AgentSessionClient`, each operation's route and
  timeout, and how HTTP failures map onto `RoutineSessionError`.

## Scope

- Owns: the transport. No credentials beyond the internal key, no retries, no
  usage accounting, no provisioning.
- Not here: the capability and its rules → `crates/agent_session`
  (`domain/routines.rs`); the routes → `crates/agent_session/src/inbound/routine_sessions.rs`.

## Boundaries

- Depends on: `agent_session` (domain only), `macro_authorization` (the
  internal key header), `macro_tower_layers` (trace propagation).
- Used by: `services/agent_harness_service` (routines' scheduler),
  `services/factory_service` (the software factory's agent Steps).

## Checks

- `cargo test -p agent_harness_service_client`

## Gotchas

- Prepare, prompt and follow-up are never replayed after an ambiguous
  failure: the client maps them to `OperationFailed` and
  `PromptDeliveryUnknown` and leaves
  recovery to the caller.
- A session its runtime refused comes back as `Refused` with the runtime's
  reason, read from the error body; it fails the same way when sent again.
- A connection the harness refused is not ambiguous: nothing reached it, so
  every command answers `RuntimeUnavailable`, which a caller may send again.
- `exec` waits as long as the command may run (30 minutes) and a margin: a
  command runs again when sent again.
  Only a direct connection is refused; through a gateway, a harness that is
  down answers 502 or 503, and a prompt stays `PromptDeliveryUnknown`.
