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
  one ends the Run for a person, two when unset; zero offers agents no
  question), `FACTORY_GATE_MODEL` (optional: the model, by the id Macro's
  model router knows, that reads a person's words on a Gate and stands in
  for the owner; Claude Haiku 5.5, `anthropic/claude-haiku-5-5`, when
  unset),
  `FACTORY_GATE_REPLY_THRESHOLD` and `FACTORY_STAND_IN_THRESHOLD` (optional:
  how sure, in percent, the model must be before words answer a Gate, 85,
  and before the stand-in answers one, 90), `FACTORY_STAND_IN_WINDOW_SECS`
  (optional: how long the owner may override the stand-in, 600),
  `FACTORY_EVALUATION` (optional: the Scorers, Experiments and Benchmarks,
  as JSON, of teams that name no Factory Definition, whose own
  `scorers.json` holds theirs; none when unset) and `FACTORY_SCORER_MODEL` (optional: the model
  Scorers judge with, Claude Haiku 5.5 when unset; an id past 200 bytes
  stops the service from starting).
- `src/definition_check.rs`: the `factory_definition_check [folder]`
  command, which reads a Factory Definition from a checkout of its
  repository as a Run would, and prints the first problem it finds with
  its file (exit 1). A Definition repository's checks run it.
- `src/gate_replay.rs`: the `factory_gate_replay` command, which replays
  the Gate model over the Gates people answered in `DATABASE_URL`'s
  records (`FACTORY_GATE_REPLAY_LIMIT`, 500 by default) and prints how
  often it agrees and picks wrong at each threshold.

## Scope

- Owns: wiring `factory_engine` and `factory_records` into a process, and
  the insights and evaluation routers beside the Run API (its OpenAPI merged into the Run
  API's).
- Not here: Flows, the workflow and the API's handlers → `crates/factory_engine`;
  the Run tables → `crates/factory_records`.
- Callers: people, with their own login or user API key, internal
  services, with the internal API key and an acting user
  (`x-internal-macro-user-id`), and agent sessions acting for a person
  from code mode (the Orchestrator, through the SDK's `macro.factory`).
  Either way the user owns the Runs they create. Bots are refused.

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
  `POST /runs/<id>/steps/<seq>/answer` and `{"answer": "approve"}`, or in
  words with `POST /runs/<id>/steps/<seq>/reply` and `{"reply": "Looks
  right, go ahead"}`, which the Gate model reads (it needs the model
  router's keys) or asks back on.
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
  `GET /settings/standing-rules` reads the Standing Rules a person's Runs
  are created with; `PUT` with
  `{"definition": {"plan": true, "blockers": true, "merge": false,
  "standIn": {"plan": true, "blockers": false}}}` sets the team's (`standIn`
  turns the stand-in on for those kinds of Gate; left out, it is off), `{"definition": null}` goes back to the default, and `PUT
  /settings/standing-rules/projects/<initiative id>` sets or removes a
  Project's own, the same way; `"ownerGates": true` in a definition turns
  its owner section on, `"taskThread": true` its task thread section
  (each Run on a task posts its updates to one comment thread on it), and
  `"queueLimit": 5` sets its Queue limit (1 to 1000, else 400; null or
  left out: none): the most places a person may hold. Once they hold that
  many, `POST /runs` refuses their new Run (409 `queueFull`, its message counting both kinds), a place being a Run
  waiting on them at a Gate or a question, or a Run of a Flow with a Gate
  that has not reached one yet. `GET /runs/waiting` lists the caller's
  Runs waiting at a Gate (`runs`) and those yet to reach their first
  (`starting`): the places they hold.
  `GET /runs` is the board: a page of the caller's Runs, or with
  `scope=team` their team's on tasks they can see, newest first, each with
  its `state` (`running`, `waiting`, `held`, `stopped`, `finished`);
  `state`, `flow`, `repoUrl`, `projectId` and `search` narrow it, and
  `afterCreatedAt` with `afterId` (the page's `next`) reads the next page.

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
  start and names both. The `verifier`, which checks a change against its
  request before and after its merge, takes `reviewer`'s Agents the same
  way. It always works in a session of its own, never the writer's; list
  Agents for it other than the writer's to have another agent check the
  work. `reviewer` stays required: Runs started before the
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

  A team that names a Factory Definition (`crates/factory_engine`'s
  `domain/definition.rs`) gives its Runs the Agents in its `agents.json`,
  which each Run keeps: `FACTORY_AGENTS` is then the fallback for the Runs
  of teams that name none. The agent Steps still run on `LocalAgent` when it
  is unset, Definition or not, so a deployment that runs agents sets it.

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
- The Gate model calls a model through Macro's model router, which reads
  its provider keys from the environment as every caller of the `agent`
  crate does (`ANTHROPIC_API_KEY` and the others `ModelRouter::try_from_env`
  names). Without them every call fails: the factory then asks back on
  every reply and the stand-in answers nothing, so Gates wait for a person
  as before. Its usage is recorded against each Run's owner
  (`AiFeature::FactoryGate`, not billable). A comment on a Run's task
  never answers a Gate (`crates/factory_engine/AGENTS.md` says why).
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

- Automations' routines are made through the scheduler at
  `ScheduledActionServiceUrl` (`SCHEDULED_ACTION_SERVICE_URL` overrides
  its default), with `INTERNAL_API_KEY` and the Automation's creator as the
  acting user, so the scheduler must take that key. `POST
  /automations/{id}/webhook` takes calls with no login: put it behind the
  gateway's `/factory` route as the rest of the API. An Automation's Run
  meets its creator's Queue limit in the Run API, as their own starts do.
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
- A Gate left for a person, on a Run whose owner section is on, notifies
  its owner through the notification ingress queue
  (`NotificationIngressQueue`, over SQS as every Macro producer), so the
  service needs `sqs:SendMessage` on it once it deploys. An answered Gate's
  notification is marked done through the notifications' reader over
  MacroDB, which queues the push's clearing on `NotificationQueue`, so it
  needs `sqs:SendMessage` there too; the owner's
  comments on the Run's task are read through document storage's
  `/messages/document/{id}` routes with `DOCUMENT_STORAGE_SERVICE_AUTH_KEY`,
  as the owner, and a Run whose task thread section is on posts its
  updates through `POST /messages/document/{id}` the same way, notifying
  no one, finds its thread again through the same read routes, and links
  the Run in the app at `AppServiceUrl` (`OVERRIDE_APP_SERVICE_URL` overrides it). Deploy `notification_service` and `document_storage_service`
  with the `factory_gate_waiting` kind before a factory that sends it.
- An operator clears a Main Hold no look can clear: `POST
  http://localhost:8113/internal/main-holds/clear` with the
  `x-internal-auth-key` header and `{"repoUrl":
  "https://github.com/<owner>/<name>", "baseBranch": "main"}`.
