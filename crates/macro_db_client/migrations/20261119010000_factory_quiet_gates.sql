-- A Gate that waits on a software factory Run's owner now notifies them
-- unless the Run's Orchestrator Definition turns that off (crates/factory_records):
-- the opt-out is kept as `quiet_gates` beside the definition's other names.
-- A definition without it notifies, so a team or Project that never set
-- rules, and the factory's default, do.
--
-- Notifications are on for everyone, and turning them off is new: no
-- stored definition has asked for it, so this only allows the name and
-- changes no row. A kept definition with the owner section off (a team's,
-- a Project's or a Run's) notifies from now on, a Run under way included.
--
-- Swapping each check scans its table under its ACCESS EXCLUSIVE lock,
-- which these small tables hold briefly. Give up rather than queue behind
-- a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_standing_rules,
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project', 'definition')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread', 'quiet_gates'
        ]
    );

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_standing_rules_check,
    ADD CONSTRAINT factory_team_settings_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread', 'quiet_gates'
        ]
    );

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_standing_rules_check,
    ADD CONSTRAINT factory_project_rules_standing_rules_check CHECK (
        standing_rules <@ ARRAY[
            'plan', 'blockers', 'merge', 'owner_gates', 'stand_in_plan', 'stand_in_blockers',
            'orchestrator_plan', 'orchestrator_blockers', 'task_thread', 'quiet_gates'
        ]
    );
