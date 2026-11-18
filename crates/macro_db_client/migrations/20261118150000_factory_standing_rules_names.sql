-- The Orchestrator Definition of the software factory (crates/factory_records)
-- keeps its task thread section (`task_thread`) and the kinds of Gate the
-- Orchestrator may approve (`orchestrator_plan`, `orchestrator_blockers`)
-- beside its other names. The two migrations that added them each replaced
-- the checks of the stored names with their own list; these checks allow
-- every name of both.
--
-- Swapping each check scans its table under its ACCESS EXCLUSIVE lock,
-- which these small tables hold briefly; no row is rewritten. Give up
-- rather than queue behind a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_standing_rules,
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread'
        ]
    );

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_standing_rules_check,
    ADD CONSTRAINT factory_team_settings_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread'
        ]
    );

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_standing_rules_check,
    ADD CONSTRAINT factory_project_rules_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread'
        ]
    );
