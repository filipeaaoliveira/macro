# factory_service

## What it is

The software factory's service: serves the Run API and hosts the Temporal
worker that walks each Run's Flow, in one process.

## Start here

- `src/main.rs`: composition root — config, MacroDB pool, Temporal client and
  worker, router, shutdown.
- `src/config.rs`: `DATABASE_URL`, `INTERNAL_API_KEY`, `TEMPORAL_ADDRESS`,
  `DOCUMENT_STORAGE_SERVICE_AUTH_KEY` (reading the tasks Runs work on),
  `AUTHENTICATION_SERVICE_SECRET_KEY` (telling which accounts exist),
  `FACTORY_AGENT_ID` (optional: the agent doing agent Steps).

## Scope

- Owns: wiring `factory_engine` and `factory_records` into a process.
- Not here: Flows, the workflow and the API's handlers → `crates/factory_engine`;
  the Run tables → `crates/factory_records`.
- Callers: people, with their own login or user API key, and internal
  services, with the internal API key and an acting user
  (`x-internal-macro-user-id`). Either way the user owns the Runs they create.
  Bots are refused.

## Checks

- `cargo build -p factory_service`
- `curl http://localhost:8113/health` after `just run_local`, and
  `curl https://localhost:8090/factory/health` through the local proxy.
- A Run end to end: `POST http://localhost:8113/runs` with the
  `x-internal-auth-key` header (the stack's `INTERNAL_API_KEY`), the
  `x-internal-macro-user-id` header (`macro|<email>`), and
  `{"flow": "reply", "request": "..."}` (a built-in Flow's name, which
  `GET /flows` lists), then `GET /runs/<id>`. The `plan` Flow runs the agent
  Steps `plan` and
  `write`, with a Gate between them: answer it with
  `POST /runs/<id>/steps/<seq>/answer` and `{"answer": "approve"}`.
  `POST /runs/<id>/cancel` cancels a Run. Add
  `"taskId": "<task document id>"` to link a Run to a task, or
  `PUT /runs/<id>/task` later.
  A `code` Run also takes
  `"repoUrl": "https://github.com/<owner>/<name>"`.

## Gotchas

- `FACTORY_AGENT_ID` names the agent (a bot id) whose sessions do the agent
  Steps, as each Run's owner; the owner must be allowed to use it. Unset, the
  agent Steps run on `LocalAgent`, which does no work. Locally, set it in the
  `--env-file` the stack is started with
  (`docs/development/running-locally.md`). A macrod agent's sessions need
  its machine connected, and routine apps are refused for it.
- The service tells which accounts exist with the authentication service's
  `POST /internal/get_existing_users`, so it requires the auth service's fix
  that serves that route as POST: before it, the route answered only GET,
  and every Run was refused as having no account.
- The `code` Flow acts on GitHub as each Run's owner, with the token of the
  GitHub account they linked in Settings, which the authentication service's
  `GET /internal/github_access_token` gives. Locally, linking needs a real
  GitHub OAuth app in the stack's env file (`GITHUB_CLIENT_ID`,
  `GITHUB_CLIENT_SECRET`, `docs/development/running-locally.md`): with the
  `local-` stubs no account links, and every `code` Run is refused as having
  an unreachable repository. The agent pushes the Run's branch with its
  runtime's own git credentials.

- Open for deploying: the service reaches the harness's routine sessions at
  `AgentHarnessServiceUrl`, which outside the local stack is the public
  gateway, as for every Macro caller outside the harness's process (the
  scheduler runs inside it). While the harness deploys, the gateway answers
  502 or 503, so a prompt sent then is `PromptDeliveryUnknown` and its Step
  fails for good after a minute's wait. Macro has no in-network address for
  the harness to use instead.
- Local-only: the local stack runs a Temporal dev server (`temporal`, UI on
  `localhost:8233`); no deployed environment has Temporal yet, so the service
  has no deploy inventory, Doppler project or gateway route. The local proxy
  serves it at `/factory`, where the web app reaches it.
- The API and the worker stop together: when either stops, the process
  shuts the other down and exits.
