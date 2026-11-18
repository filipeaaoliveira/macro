-- The Orchestrator in the software factory (crates/factory_records): an
-- agent session in Macro chat answering a Gate for the Run's owner.
--
-- A Gate keeps the agent session that gave its answer
-- (`agent_session_id`), so the answer reads as the Orchestrator's and is
-- left out of the owner's own answers. The Orchestrator Definition keeps
-- the kinds of Gate the Orchestrator may approve
-- (`orchestrator_plan`, `orchestrator_blockers`) beside its other names.
--
-- Adding a nullable column rewrites no rows; swapping and adding the
-- checks scans each table under its ACCESS EXCLUSIVE lock, which these
-- small tables hold briefly. Give up rather than queue behind a long
-- transaction on any of them.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_gate
    ADD COLUMN agent_session_id uuid,
    ADD CONSTRAINT factory_gate_agent_answer CHECK (
        agent_session_id IS NULL
        OR (answer IS NOT NULL AND rule IS NULL AND NOT by_stand_in)
    );

ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_standing_rules,
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers'
        ]
    );

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_standing_rules_check,
    ADD CONSTRAINT factory_team_settings_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers'
        ]
    );

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_standing_rules_check,
    ADD CONSTRAINT factory_project_rules_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers'
        ]
    );
