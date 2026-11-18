# Factory Definition

This repository is a team's Factory Definition: the files that say how the
team's software factory works. The factory reads them from the repository's
default branch when a Run starts, and the Run keeps the commit it read. A
change to them reaches new Runs once it is merged; a Run already going keeps
the Definition it started with.

## Start from this template

1. Copy this folder into a new repository, at its root.
2. In `agents.json`, replace each `bot` with the id of a Macro agent your
   team's members may use, and set its `model` and `instructions` if you want.
3. Protect the default branch: require a pull request and the `definition`
   check before a merge. Nothing else stops a push from changing the rules.
4. Check the Definition (below), then merge it.
5. In Macro, an admin of the team sets the repository in the factory's team
   settings (Factory Definition). Every member who starts Runs must be able
   to read the repository with the GitHub account they linked to Macro.

While the team names a Definition, it is the only source of the team's
Flows, Agents and rules. A Definition that does not read, or a repository a
member cannot reach, starts no Run for them: the factory never falls back to
its built-in Flows or the team's own settings. Team settings say why.

## The files

| File | What it holds |
| --- | --- |
| `factory.json` | The format's `version` (1), a `description`, and the Flows, by name. |
| `flows/{name}.json` | Each Flow `factory.json` names, in the factory's Flow format. |
| `agents.json` | The Agents (`bot`, optional `model` and `instructions`) and, for each Role, the Agents that fill it, in order. It must fill every Role a Flow names. |
| `rules.json` | Optional. `standingRules`: which Gates the factory answers on its own. `controlFiles`: the Control File patterns, which replace the factory's default whole; this template lists the default and this Definition's own files. Each left out takes the factory's default. |
| `scorers.json` | Optional. The team's Scorers, Experiments and Benchmarks, in the factory's evaluation format (below). Left out, the team has none. |
| `.macro/checks.json` | The check a Run on this repository runs: it validates the Definition. |

Secrets never go here: no field takes an API key or a token, and a file that
holds something shaped like one does not read.

## The rules this template sets

`rules.json` turns on:

- `blockers`: the factory sends a review round's Blockers to be fixed, for at
  most two rounds in a row; later Blockers wait for a person.
- `ownerGates`: the owner's comments on the Run's task reach the next agent
  Step, and Blockers a rule's fix left unchanged wait for them.
- `taskThread`: a Run on a task posts its main updates to the task.

A Run on this repository always counts this Definition's own files as
Control Files, and its change to one is Tampered even when its request names
the file: a person approves every change a Run makes to them before it
merges, whatever `merge` says. So `controlFiles` lists at most 195
patterns, leaving room for the 5 the Run adds.

It leaves off:

- `quietGates`: every Gate no rule answers notifies the Run's owner. Set it
  to `true` to stop those notifications; the Gates still wait on the owner.

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
