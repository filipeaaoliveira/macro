-- The Factory Definition of the software factory (crates/factory_records):
-- the files in a GitHub repository a team names, changed by pull requests,
-- that hold its Flows, Agents, rules and Scorers. A Run reads them at the
-- commit the repository's default branch is at when it starts, and keeps
-- that commit and the Agents it read, so a change merged later never
-- changes a Run under way.
--
-- Adding nullable columns rewrites no rows; adding the check, and replacing
-- the Standing Rules' check, scan factory_run under its ACCESS EXCLUSIVE
-- lock, which this small table holds briefly. The new table references
-- "team", so this takes a lock on it until the transaction ends. Give up
-- rather than queue behind a long transaction on either.
SET LOCAL lock_timeout = '5s';

-- The repository a team names as its Factory Definition. No row: the team
-- names none, and its Runs take the team's settings and the factory's
-- built-in Flows and Agents, as before. It goes with the team.
CREATE TABLE factory_team_definition (
    team_id uuid PRIMARY KEY REFERENCES team (id) ON DELETE CASCADE,
    repo_url text NOT NULL CHECK (
        length(repo_url) <= 300
        AND repo_url ~ '^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'
    ),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- The Definition a Run started under: its repository, the commit read, and
-- the Agents it holds, which the Run's agent Steps take. All null for a Run
-- whose team named no Definition, and for every Run created before this.
ALTER TABLE factory_run
    ADD COLUMN definition_repo_url text,
    ADD COLUMN definition_commit text,
    ADD COLUMN definition_agents jsonb;

ALTER TABLE factory_run
    ADD CONSTRAINT factory_run_definition CHECK (
        (definition_repo_url IS NULL) = (definition_commit IS NULL)
        AND (definition_commit IS NULL) = (definition_agents IS NULL)
        AND definition_commit ~ '^[0-9a-f]{40}$'
    );

-- A Run's Standing Rules may come from its team's Definition.
ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_standing_rules,
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project', 'definition')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread'
        ]
    );
