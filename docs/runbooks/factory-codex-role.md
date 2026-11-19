# A factory Role on Codex

How to run one factory Role (for example the reviewer) on Codex while the
other Roles stay on Claude Code, on a test bench where the factory's agents
run on the owner's own machine through macrod. What the engine does with a
Codex Agent is in [`crates/factory_engine/AGENTS.md`](../../crates/factory_engine/AGENTS.md)
("A Role on Codex").

## The runtime and its login

- **Runtime:** macrod on the owner's machine, with the Codex preset
  (`macrod service --agent codex`). It runs the installed `codex` CLI through
  the `@agentclientprotocol/codex-acp` adapter, which macrod installs and
  points at the installed CLI (`CODEX_PATH`). The `herdr-codex` preset (Codex
  in a Herdr tab) works too, but the plain `codex` preset is the one the
  factory's usage-limit handling is tested against.
- **Login:** the owner's own Codex CLI login on that machine, made with
  `codex login` (their ChatGPT account) before macrod starts. Nothing of it
  reaches Macro: macrod and the adapter only run the CLI, which reads its own
  login from `~/.codex`. Never give Macro, macrod's config or the factory an
  OpenAI API key or a subscription token for this.
- **Not Codex's cloud agent:** the Codex bot in Macro's agent list
  (`00000000-0000-0000-0000-00000000c0de`) runs in OpenAI's cloud, works in
  its own environment and cannot hand a Step's result back; the factory
  refuses an Agent on it.

One macrod serves one runtime. A machine that already serves Claude Code
for the factory runs Codex as a second profile beside it:
`macrod service --profile codex --agent codex --workspace <path>`. Each
profile pairs as its own machine (`<hostname> · codex`).

## Set it up

1. On the machine: `codex --version` (any recent CLI), then `codex login`
   and check `codex` answers a prompt in a terminal.
2. Start macrod with the Codex preset (above) and pair it from Macro's
   Settings → Machines. Turn on permission bypass for it there: a factory
   Step's session never grants a permission prompt itself, so a session
   that asks waits until the Step's time runs out.
3. In Macro, create an agent whose runtime is that machine, and copy its
   id: that is the Agent's `bot`.
4. Name it in the agent definition, `FACTORY_AGENTS` or the team
   Definition's `agents.json`, with a model `codex` lists (for example
   `gpt-5.5`; left out, Codex's default), and list it first for the Role it
   takes, with a Claude Code Agent after it as the backup:

   ```json
   {
     "agents": {
       "primary": { "bot": "<Claude Code machine's agent id>" },
       "second-opinion": { "bot": "<Codex machine's agent id>", "model": "gpt-5.5" }
     },
     "roles": {
       "planner": ["primary"],
       "writer": ["primary"],
       "reviewer": ["second-opinion", "primary"],
       "fixer": ["primary"]
     }
   }
   ```

   `reviewer`'s Agents also fill the review panel's Roles and the
   `verifier` unless they are listed. To put only one panel Role on Codex,
   list it (`"correctness-reviewer": ["second-opinion", "primary"]`).
   The service checks the definition as it starts (`FACTORY_AGENTS`), and
   `factory_definition_check` and team settings check a Definition.

## A live code Run with a Step on each

1. Both machines are connected (Settings → Machines shows them online) and
   their workspaces reach the Run's repository with git credentials that
   may push to it.
2. Start a `code` Run on a small task of the bench repository.
3. Watch the Run view: the review panel's Steps record their Agent
   (`second-opinion` for the Codex Role); open a Step's Terminal to see the
   Codex session work. The writer's Steps record `primary`.
4. The Run goes through the review, `accept`, CI and the merge Gate; answer
   the Gate and it ends `success` once merged.
5. Check what each Step recorded: the Codex Step's Agent, no Agent passed
   over (unless its usage limit was hit, below), and its hand back as the
   review's result.

## What to expect from Codex

- **Results:** the agent hands its result back with
  `macro.factory.handBack` from code mode, as Claude Code does; Codex reaches
  Macro's tools through the same MCP servers macrod forwards to the session.
  A turn that ends without one is reminded once, then its Step asks the
  owner (`no_result`) with Codex's last message.
- **Usage limit:** when the owner's Codex plan runs out, the adapter ends the
  turn with Codex's "usage limit" error. The Step's Agent is then out of
  quota: a turn that called no tool hands the Step to the Role's next Agent
  (recorded passed over as `usage_limit`), one that did work ends the Step
  for infrastructure (the Run ends `infrastructure`, not `failed`), and the
  owner's next Steps skip the Codex Agent for 15 minutes. The session shows
  Codex's own message, with when the limit resets.
- **Usage and cost:** Codex signed in with the owner's own login records no
  token use in Macro, as Claude Code under its own login does. A token
  budget counts only the Steps that did; Insights counts the Codex Steps
  among the unmeasured ones and gives no cost or tokens per merged pull
  request while any Run of the period has an unmeasured Step.
- **Checks:** on the owner's own (paired) machine the engine runs no
  command, so the agent runs the repository's checks itself in later
  prompts of its session, as Claude Code does there. Only a Codex Step that
  writes or fixes meets them; a reviewer on Codex runs none.
