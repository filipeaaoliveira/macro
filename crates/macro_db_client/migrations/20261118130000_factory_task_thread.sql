-- The Orchestrator Definition of the software factory (crates/factory_records)
-- gains a task thread section (`task_thread`): with it on, a Run on a task
-- posts its main updates to one comment thread on the task. A definition is
-- kept as the names it turns on, so the section is kept as `task_thread`
-- beside the other names, in each column that keeps one. Off by default:
-- no stored definition names it.
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
            'task_thread'
        ]
    );

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_standing_rules_check,
    ADD CONSTRAINT factory_team_settings_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'task_thread'
        ]
    );

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_standing_rules_check,
    ADD CONSTRAINT factory_project_rules_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'task_thread'
        ]
    );
