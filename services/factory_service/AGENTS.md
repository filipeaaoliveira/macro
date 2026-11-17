# factory_service

## What it is

The software factory's service: serves the Run API and hosts the Temporal
worker that walks each Run's Flow, in one process.

## Start here

- `src/main.rs`: composition root — config, MacroDB pool, Temporal client and
  worker, router, shutdown.
- `src/config.rs`: `DATABASE_URL`, `INTERNAL_API_KEY`, `TEMPORAL_ADDRESS`,
  `DOCUMENT_STORAGE_SERVICE_AUTH_KEY` (reading the tasks Runs work on, and
  filing a Main Hold's task as the Run's owner),
  `AUTHENTICATION_SERVICE_SECRET_KEY` (telling which accounts exist),
  `FACTORY_AGENTS` (optional: the agent definition, the Agents doing agent
  Steps), `FACTORY_REVIEW_ROUNDS` (optional: the most rounds a `code` Run's
  review panel runs, three when unset), `FACTORY_QUESTIONS` (optional: the
  most questions each agent Step may ask the Run's owner before the next
  one ends the Run for a person, two when unset).

## Scope

- Owns: wiring `factory_engine` and `factory_records` into a process, and
  the insights router beside the Run API (its OpenAPI merged into the Run
  API's).
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
  `GET /settings/control-files` reads the Control File patterns a person's
  Runs are checked against, their team's or the default; `PUT` with
  `{"patterns": [...]}` sets the team's, and `{"patterns": null}` goes back
  to the default. `PUT` takes only an admin or the owner of the team, in
  their own signed-in session, as a Gate's answer does.

## Gotchas

- `FACTORY_AGENTS` is the factory's agent definition, as JSON: `agents`
  names each Agent (`bot`, the agent whose sessions it runs in, which the
  Run's owner must be allowed to use; an optional `model` and
  `instructions`), and `roles` lists, for each Role, the Agents that fill it
  in order (`factory_engine`'s `domain/agents.rs`). The service refuses to
  start when a Role a built-in Flow names has no Agent. The `code` Flow's
  review panel names `correctness-reviewer`, `scope-reviewer` and
  `tests-reviewer`: each the definition does not list takes `reviewer`'s
  Agents, so a definition written for one reviewer keeps working, and lists
  one to give it Agents of its own. Without either, the service refuses to
  start and names both. `reviewer` stays required: Runs started before the
  review panel review with it until they end. For example, with a
  backup for planning when the first Agent's quota runs out:

  ```json
  {
    "agents": {
      "primary": { "bot": "<agent id>", "model": "<model>" },
      "backup": { "bot": "<another agent id>", "instructions": "<text>" }
    },
    "roles": {
      "planner": ["primary", "backup"],
      "writer": ["primary"],
      "reviewer": ["backup"],
      "fixer": ["primary"]
    }
  }
  ```

  When moving off `FACTORY_AGENT_ID`, make that bot the first Agent of every
  Role until the Runs started before have ended: a Step that opened its
  session before Steps recorded their Agent and bot is stopped, deleted or
  retried as the first Agent of its Role.

  Unset, the agent Steps run on `LocalAgent`, which does no work, unless the
  retired `FACTORY_AGENT_ID` is set: then the service refuses to start and
  names `FACTORY_AGENTS`. It is optional and the service has no Doppler
  project yet, so it has no Doppler entry or boot stub. Locally, set it in the
  `--env-file` the stack is started with
  (`docs/development/running-locally.md`). A macrod agent's sessions need
  its machine connected, and routine apps are refused for it.
- A review round makes each finding outside the diff it reviewed a task of
  the Run's owner, in the project of the Run's task, through document
  storage's `POST /documents/create_task` with `DOCUMENT_STORAGE_SERVICE_AUTH_KEY`
  in the standard internal headers and the owner as the acting user, and an
  id it picks, so a retry finds the task: the owner must be able to add
  tasks to that project, and document storage must take the id (deploy it
  first). Whether the owner may add to the project is read from entity
  access before the task is asked for (document storage answers a project
  they may not add to with the same 401 as a key it does not accept). A
  task it refuses, or one in a project the owner may not add to, is not
  made; its finding stays on the round as a follow-up.
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
- The worker also runs the watches of the base branches Runs merge to
  (`factory_main_watch`), on the same task queue. A Main Hold's task is
  filed through the document storage service's `POST /documents/create_task`,
  read through `GET /properties/entities/DOCUMENT/{id}` and commented on
  through `POST /documents/{id}/comments/text`, as the Run's owner with the
  internal key, so the service needs that service's public routes reachable
  at `DocumentStorageServiceUrl`.
- An operator clears a Main Hold no look can clear: `POST
  http://localhost:8113/internal/main-holds/clear` with the
  `x-internal-auth-key` header and `{"repoUrl":
  "https://github.com/<owner>/<name>", "baseBranch": "main"}`.
