# Factory Definition

This repository is a team's Factory Definition: the files that say how the
team's software factory works. The factory reads them from the repository's
default branch when a Run starts, and the Run keeps the commit it read. A
change to them reaches new Runs once it is merged; a Run already going keeps
the Definition it started with.

## Customize the template

Every value a team must or may change, file by file. "Left as is" says what
happens when you do not change it.

Outside the files, all must be done:

- The repository: copy this folder to the root of a new GitHub repository.
- Branch protection: require a pull request on the default branch, and a
  status check that runs `factory_definition_check .`. The factory reports
  no check to GitHub, so add a CI job that runs it. Left as is, a push
  changes the team's rules unreviewed, and the Factory definition setting
  says so.
- The validator: `factory_definition_check` on the `PATH` of the CI runner
  and of every machine a Run's agent works on (Check the Definition, below).
  Left as is, the `definition` check fails, and a Run on this repository
  never gets past its checks.
- The team setting: an admin of the team names the repository in team
  settings, Factory definition. Left as is, the team's Runs take Macro's
  built-in Flows and Agents and the team's own settings.
- Linked GitHub accounts: each member who starts Runs links GitHub in
  Settings, Connected. That account must read this repository, and reach
  and push to each repository a `code` Run works on. Left as is, their Runs
  do not start, and New run says why.

`agents.json`:

- `agents.primary.bot` and `agents.backup.bot` (must): the id of a Macro
  agent each member who starts Runs may use. The zero ids name no agent,
  and the check does not refuse them: replace them yourself.
- `model` (may): left out, the agent's own default model.
- `instructions` (may): what every session of the Agent is told before the
  Step's prompt. Left as is, `primary` is told to work in small commits.
- `roles` (may): the Agents of each Role, in order. A Step works with the
  first; the next takes over only when Macro refuses the first for lack of
  quota. Keep `planner`, `writer`, `reviewer` and `fixer` filled: a
  self-improvement Run needs them. A review panel's Role and `verifier`,
  when not listed, take `reviewer`'s Agents. Left as is, `primary` plans,
  writes and fixes, and `backup` reviews and verifies.
- `standing` (may): the Standing Roles (below). Left out, any of your
  agents may start, cancel and link Runs, and answer plan and Blockers
  Gates where `rules.json` lets it.

`factory.json`:

- `version` (keep): 1, the only format this factory reads.
- `description` (may): what the Definition is for.
- `flows` (may): the Flows members can start. Remove one you do not use,
  with its file. No Flow may be named `benchmark` or `improve`. Left as is,
  `code`, `plan` and `reply`.

`flows/{name}.json` (may):

- `max_visits`: how many times a Run comes back to an entry before it is
  handed to you. Left as is, 3 for `plan`, `review`, `accept`, `ci-fix`
  and `merge-gate`.
- A Gate's `question`: the words the owner reads.
- Keep the merge Gate on the `ready_to_merge` entry: the check refuses a
  Flow that merges another way.

`rules.json` (may; left out whole, the factory's defaults apply):

- `standingRules.plan`, `blockers` and `merge`: the three must all be there
  when `standingRules` is. Left as is, a person approves each plan and each
  merge, and the factory sends Blockers to be fixed, for at most two rounds
  in a row.
- `ownerGates` and `taskThread`: left as is, both on (The rules this
  template sets, below).
- `quietGates`: left out, every Gate no rule or stand-in answers notifies
  the Run's owner.
- `queueLimit`: from 1 to 1000, how many of a person's Runs may wait on
  them before no new Run of theirs starts. Left out, no limit.
- `standIn`: `{"plan": true, "blockers": true}` lets a small model answer
  those Gates as the owner would. Left out, it answers none.
- `orchestrator`: the same two kinds, which the owner's agent in Macro chat
  may approve for them. Left out, it approves none; it may always send work
  back.
- `controlFiles`: the Control File patterns. A list replaces the factory's
  default whole, holds at most 195 patterns, and an empty list checks no
  file. Left as is, the factory's default and this Definition's own files.

`scorers.json` (may):

- `samplePercent`: the share of finished `code` Runs the
  `does-what-was-asked` Scorer judges. Left at 0, it judges none, and only
  Runs a person marks flawed count as failures for self-improvement.
- `experiments` and `benchmarks`: left empty, the team has none.
- `selfImprovement`: left out, on, starting a Run at 25 pending failures or
  when the oldest is 7 days old.

`.macro/checks.json` (keep): the `definition` check every Run on this
repository runs. It needs the validator on the `PATH`, as above.

## Use the Definition

While the team names a Definition, it is the only source of the team's
Flows, Agents and rules. A Definition that does not read, or a repository a
member cannot reach, starts no Run for them: the factory never falls back to
its built-in Flows or the team's own settings. Team settings say why.

## Your first Run

From this template to a first merged pull request:

1. Copy this folder to the root of a new GitHub repository.
2. Replace the two `bot` ids in `agents.json`, and anything else from
   Customize the template you want now.
3. Check it: `factory_definition_check .` prints `.: the definition reads:
   flows code, plan, reply, 1 scorers, 0 experiments, 0 benchmarks`.
4. Protect the default branch, and merge.
5. An admin names the repository in team settings, Factory definition, and
   saves. Macro reads it first and refuses one that does not read, saying
   why. It then shows the commit Runs read, and whether the default branch
   requires a pull request.
6. Pick a repository for the Run to change, one your linked GitHub account
   can push to. Its `.macro/checks.json` holds the checks `write` and `fix`
   run on the change.
7. Open a small task. In its Runs section, choose New run, the `code` Flow
   and that repository, and write the request.
8. Answer the plan Gate, then the merge Gate (below).
9. Read the Run in Insights (below).

### A sample task

A request that names its Done-when clauses and the files it asks for:

```text
Add a --version flag to the command-line tool.

Done when:
1. `tool --version` prints the version in Cargo.toml and exits 0.
2. tests/version.rs runs `tool --version` and checks what it prints.
```

What each Step of the `code` Flow does with it:

| Step | Who | What it produces |
| --- | --- | --- |
| `plan` | `primary` | A plan, ending with one acceptance check per Done-when clause: a command that exits 0 only when the clause holds. Then the plan Gate. |
| `write` | `primary` | Commits on the Run's branch, `factory/{run id}`, then the repository's checks, run up to 3 rounds. |
| `review` | `backup` | Three reviewers (correctness, scope, tests) review the change at once. Blockers go to `fix`; a finding outside the change becomes a follow-up task. |
| `fix` | `primary` | Commits that fix the Blockers, checked and reviewed again. |
| `accept` | `backup` | The verifier checks each Done-when clause on the branch, and the factory runs the plan's acceptance checks. |
| `publish` | the factory | The pull request, opened with the owner's linked GitHub account. |
| `ci` | the factory | Waits for the pull request's checks. Failing checks go to `ci_fix`, then back to `review`. |
| `ready_to_merge` | the factory | The merge Gate, once the checks pass. |
| `merge` | the factory | Merges once the base branch's required checks pass. The Run ends Done. |

The Gates the owner sees, in order:

1. The plan Gate: approve, or revise with a note. Nothing is written
   before you approve.
2. A Blockers Gate, only when the rules leave Blockers to you: a review
   round finds the same Blockers a rule's fix left unchanged (`ownerGates`).
   Fix sends them to `fix`; stop leaves the branch to you.
3. The merge Gate: approve, or revise with a note.

The `blockers` rule fixes the first two rounds' Blockers. Blockers the
third review round still finds, or a third failing acceptance round, end
the Run Handed off, with the branch left to you.

An agent may also ask you a question mid-Step. Each Gate no rule answers
notifies you, and a Run on a task posts its updates to the task.

### First-run checks

1. The plan Gate comes first, and no commit lands on the branch before
   you approve it.
2. Nothing merges until you approve the merge Gate.
3. A change to a Control File the request does not name (`Cargo.toml`,
   say) is held: the merge Gate lists it under Control files changed, and
   only the Approve button, which lists it, approves it.
4. The Run view shows this repository and the commit the Run read.

A Run that ended Done with a wrong result: mark it flawed. It then counts
as a failure for self-improvement (below).

### Read Insights

Insights (Factory, Insights) shows the team's Runs by Flow and period:
their success rate, how long each Step took, where Runs stop, and the
tokens the agents used. Its Scorers, Experiments and Benchmarks sections
fill once `scorers.json` has them.

Change one setting at a time, and repeat a Run before you compare: one
Run says little. To compare Agents on the same kind of work, use an
Experiment, which splits a Flow's Runs across arms. To compare a change on
fixed tasks, use a Benchmark, which replays them on one setup and scores
each trial with the tokens it used.

## The files

| File | What it holds |
| --- | --- |
| `factory.json` | The format's `version` (1), a `description`, and the Flows, by name. |
| `flows/{name}.json` | Each Flow `factory.json` names, in the factory's Flow format. |
| `agents.json` | The Agents (`bot`, optional `model` and `instructions`) and, for each Role, the Agents that fill it, in order. It must fill every Role a Flow names. Optional `standing`: the Standing Roles (below). |
| `rules.json` | Optional. `standingRules`: which Gates the factory answers on its own. `controlFiles`: the Control File patterns, which replace the factory's default whole; this template lists the default and this Definition's own files. Each left out takes the factory's default. |
| `scorers.json` | Optional. The team's Scorers, Experiments and Benchmarks, in the factory's evaluation format, and when its failures start a self-improvement Run (below). Left out, the team has none, and the defaults apply. |
| `.macro/checks.json` | The check a Run on this repository runs: it validates the Definition. |

A repository a Run works on may also hold `.macro/setup.json`: the commands
that prepare a fresh checkout (install dependencies, generate files), each a
`name`, a `run` and an optional `timeout`, and plain `env` values. The
factory reads it from the Run's base, runs it before each agent session's
first prompt and before each checks round's checks, and counts it as a
Control File by default. It holds no secrets.

Secrets never go here: no field takes an API key or a token, and a file that
holds something shaped like one does not read.

## The rules this template sets

`rules.json` turns on:

- `blockers`: the factory sends a review round's Blockers to be fixed, for at
  most two rounds in a row. With the `code` Flow's three review rounds,
  Blockers the third round finds end the Run, and leave the branch to a
  person.
- `ownerGates`: the owner's comments on the Run's task reach the next agent
  Step, and Blockers a rule's fix left unchanged wait for them.
- `taskThread`: a Run on a task posts its main updates to the task.

A Run on this repository always counts this Definition's own files as
Control Files, and its change to one is Tampered even when its request names
the file: a person approves every change a Run makes to them before it
merges, whatever `merge` says. So `controlFiles` lists at most 195
patterns, leaving room for the 5 the Run adds.

It leaves off:

- `quietGates`: every Gate no rule or stand-in answers notifies the Run's
  owner. Set it to `true` to stop those notifications; the Gates still wait
  on the owner.
- `plan`: a person approves each plan.
- `merge`: a person merges each pull request. Turn it on only once you trust
  the Flow's checks and review.

## Scorers, Experiments and Benchmarks

`scorers.json` holds the team's evaluation, as the factory's
`FACTORY_EVALUATION` does for teams that name no Definition:

- `scorers`: each a `name`, the `instructions` a small model judges a
  finished Run by (from 0 to 1), the `flows` whose Runs it judges (every
  Flow of this Definition when left out), and `samplePercent`, the share of
  those Runs it judges, from 0 to 100.
- `experiments`: each a `name`, the `flow` whose Runs on a task it splits,
  and at least two `arms`, each a `name`, an optional `weight` and the
  `agents` it gives Roles in place of `agents.json`'s first.
- `benchmarks`: each a `name`, the `repository` its trials work on, its
  `tasks` (a `name`, a `request` and an optional pinned `baseCommit`), and
  the `scorers` that judge its trials.

The Flows and Agents they name must be this Definition's own. A
Benchmark's trials walk the factory's own `benchmark` Flow (plan, then
write, asking no one), which no Flow here may be named: with Benchmarks,
`agents.json` must fill its `planner` and `writer` Roles. A Run keeps
the Scorers it started with, as it keeps the commit. Every judgment is a
model call: this template's Scorer judges no Run (`samplePercent` 0) until
you raise it.

## Standing Roles

An agent you chat with in Macro can act on your Runs for you from its code:
start a Run, cancel one, link one to a task, and answer a plan or Blockers
Gate where `rules.json` lets it. This template has no `standing` section,
so any of your agents may do each of these.

To say which agents may do which, add a `standing` section to
`agents.json`. Each Standing Role names the acts it allows (`acts`) and the
Agents whose bots fill it (`agents`). An agent whose bot fills no Standing
Role that allows an act is refused that act, and the refusal names the
Standing Roles that allow it. This holds for an agent's code in a chat
session and for code signed for an agent outside one, such as a routine
that runs for an agent. Your own API key, your own MCP client (code mode
included), and a routine that runs as you with no agent, act as you:
Standing Roles do not limit them. For example,
beside `agents` and `roles`:

```json
"standing": {
  "intake": {
    "description": "Starts runs from requests and links them to tasks.",
    "acts": ["start", "link"],
    "agents": ["primary"]
  },
  "gatekeeper": {
    "description": "Approves plans and sends Blockers to be fixed.",
    "acts": ["answer_plan", "answer_blockers"],
    "agents": ["backup"]
  },
  "operator": { "acts": ["cancel"], "agents": ["primary", "backup"] }
}
```

The acts are `start`, `cancel`, `link`, `answer_plan` and
`answer_blockers`, and no others. None merges, and a Definition cannot add
one: a person approves each merge Gate, or the `merge` Standing Rule merges
once the checks pass. Steering a running Step stays yours, in person. A
Standing Role decides which agent may answer a kind of Gate; `rules.json`
still decides whether an agent may approve that kind at all. Each Run keeps
the Standing Roles it started with, and shows which agent did each act, as
which Standing Role.

An empty section, `"standing": {}`, allows no agent any of these acts;
leave the section out to allow them all. `"standing": null` is refused.

The check refuses an act not in that list, any act that names a merge, a
Standing Role with no act or no Agent, an Agent `agents` does not define,
and a name given twice.

## Self-improvement

The factory learns from its failures here. A failure is a Scorer's
judgment below 0.5, or a Run its owner marked flawed, of a Run that started
under this Definition. The factory keeps each one until a pull request
against this repository addresses it. Once the pending failures reach
`selfImprovement.failures`, or the oldest is `selfImprovement.oldestDays`
old, the factory starts one self-improvement Run against this repository.
It walks the factory's own `improve` Flow, which no Flow here may be named,
so `agents.json` must fill its `planner`, `writer`, `reviewer` and `fixer`
Roles. The Run is owned by the admin who named the Definition in Macro. Its
request groups the failures, and its pull request lists them under
"Regressions addressed". A team has at most one such Run open at a time.

A person approves its plan and merges it, whatever it changes: no
standing rule, stand-in or Orchestrator answers its plan or merge, and its
changes to this Definition's files are Tampered too. Once it merges, its
failures are addressed. Its pull request closed unmerged, or its Run
cancelled by its owner in person, rejects it: its failures are dismissed
for good, and new ones still count. One that ends without merging
otherwise (no pull request, cancelled by an agent, or refused) sends them
back. After either, the next Run waits a day, twice
as long after each more in a row, up to a week, until one merges. The admin who named the Definition must still be a
team admin for a Run to start. Thresholds are read from this repository as
it is now.

```json
"selfImprovement": { "enabled": true, "failures": 25, "oldestDays": 7 }
```

Those are the defaults when `selfImprovement` is left out. `failures` is
from 1 to 1000, and `oldestDays` from 1 to 365. `"enabled": false` turns it
off: failures are still kept, and count once it is on again.

## Check the Definition

Run the factory's validator on a checkout of this repository:

```sh
factory_definition_check path/to/definition
```

It prints the first problem it finds, with the file it is in, and exits 1
when there is one. Build it from the Macro repository with
`cargo build -p factory_service --bin factory_definition_check`. The
`definition` check in `.macro/checks.json` runs it by name, so it must be on
the `PATH` of every machine that runs this repository's checks: the CI
runner, and the machine a factory Run's agent works on.

A merge needs a merge Gate's approval: a Flow whose way into its `merge`
Step skips the `ready_to_merge` entry's merge Gate, or that gives that
Gate another kind, does not read.
