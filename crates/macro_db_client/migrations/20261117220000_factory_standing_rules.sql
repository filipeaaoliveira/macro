-- Standing Rules in the software factory (crates/factory_records): which
-- Gates the factory answers on its own (a Rule Answer) instead of waiting
-- for a person. A team sets them for its members' Runs, and may set them
-- again for one Project (an initiative); a Run keeps the rules it was
-- created with.
--
-- The new columns change factory_run, factory_gate and
-- factory_team_settings, and the new table references team and
-- initiative, so this takes locks on them until the transaction ends.
-- Adding nullable columns rewrites no rows, and dropping a NOT NULL only
-- changes the catalog; adding the checks scans factory_run and
-- factory_gate under their ACCESS EXCLUSIVE lock, which these small tables
-- hold briefly. Give up rather than queue behind a long transaction on any
-- of them.
SET LOCAL lock_timeout = '5s';

-- The Gate kinds the Run's rules decide (`plan`, `blockers`, `merge`), as
-- they were when it was created, so a change to its team's or its
-- Project's rules later does not change a Run under way; where they came
-- from (`default`, `team` or `project`); and the Project, for a Project's
-- rules. All null for a Run created before Runs kept rules: no rule
-- answers its Gates. The Project id is a record, not a reference: it
-- outlives the initiative.
ALTER TABLE factory_run
    ADD COLUMN standing_rules text[],
    ADD COLUMN standing_rules_source text,
    ADD COLUMN standing_rules_project_id uuid;

ALTER TABLE factory_run
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY['plan', 'blockers', 'merge']
    );

-- The rule that answered the Gate, when a rule did rather than a person:
-- such an answer has no author, no note and approves no Control Files.
ALTER TABLE factory_gate
    ADD COLUMN rule text;

ALTER TABLE factory_gate
    ADD CONSTRAINT factory_gate_rule CHECK (
        rule IS NULL
        OR (
            answer IS NOT NULL
            AND user_id IS NULL
            AND note IS NULL
            AND approved_control_files IS NULL
            AND rule IN ('approve_plan', 'fix_blockers', 'merge_when_green')
        )
    );

-- A team's own Standing Rules, beside its Control Files: the Gate kinds
-- its rules decide. A row may now hold either setting alone: null is the
-- factory's default for that setting, and a row with neither is removed.
ALTER TABLE factory_team_settings
    ALTER COLUMN control_files DROP NOT NULL,
    ADD COLUMN standing_rules text[]
        CHECK (standing_rules <@ ARRAY['plan', 'blockers', 'merge']);

-- A team's Standing Rules for the Runs whose task is in one Project (an
-- initiative), in place of the team's own. It goes with the team or the
-- initiative.
CREATE TABLE factory_project_rules (
    team_id uuid NOT NULL REFERENCES team (id) ON DELETE CASCADE,
    project_id uuid NOT NULL REFERENCES initiative (id) ON DELETE CASCADE,
    standing_rules text[] NOT NULL
        CHECK (standing_rules <@ ARRAY['plan', 'blockers', 'merge']),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, project_id)
);

-- Deleting an initiative finds its rules without scanning every team's.
CREATE INDEX factory_project_rules_project_id_idx ON factory_project_rules (project_id);
