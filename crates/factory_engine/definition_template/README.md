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

## The files

| File | What it holds |
| --- | --- |
| `factory.json` | The format's `version` (1), a `description`, and the Flows, by name. |
| `flows/{name}.json` | Each Flow `factory.json` names, in the factory's Flow format. |
| `agents.json` | The Agents (`bot`, optional `model` and `instructions`) and, for each Role, the Agents that fill it, in order. It must fill every Role a Flow names. |
| `rules.json` | Optional. `standingRules`: which Gates the factory answers on its own. `controlFiles`: the Control File patterns. Each left out takes the factory's default. |
| `scorers.json` | Optional. The Scorers, each a `name`, a `description` and its `settings`. |
| `.macro/checks.json` | The check a Run on this repository runs: it validates the Definition. |

Secrets never go here: no field takes an API key or a token, and a file that
holds something shaped like one does not read.

## The rules this template sets

`rules.json` turns on:

- `blockers`: the factory sends a review round's Blockers to be fixed, for at
  most two rounds in a row; later Blockers wait for a person.
- `ownerGates`: a Gate no rule answers notifies the Run's owner, and their
  comments on the Run's task reach the next agent Step.
- `taskThread`: a Run on a task posts its main updates to the task.

It leaves off:

- `plan`: a person approves each plan.
- `merge`: a person merges each pull request. Turn it on only once you trust
  the Flow's checks and review.

## Check the Definition

Run the factory's validator on a checkout of this repository:

```sh
factory_definition_check path/to/definition
```

It prints the first problem it finds, with the file it is in, and exits 1
when there is one. Build it from the Macro repository with
`cargo build -p factory_service --bin factory_definition_check`.
