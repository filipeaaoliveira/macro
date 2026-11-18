-- A Gate that waits on a software factory Run's owner now notifies them
-- unless the Run's Orchestrator Definition turns that off (crates/factory_records):
-- the opt-out is kept as `quiet_gates` beside the definition's other names.
-- A definition without it notifies, so a team or Project that never set
-- rules, and the factory's default, do.
--
-- Until now only a definition with its owner section on (`owner_gates`)
-- notified. A stored definition with the section off cannot tell whether
-- someone turned it off or left it as it was, so each one keeps its owner
-- unnotified: it gets `quiet_gates`, and an admin may turn notifications
-- on. A Run keeps the definition it was created with, so Runs get the same,
-- and a Run under way notifies as it did.
--
-- Swapping each check scans its table under its ACCESS EXCLUSIVE lock,
-- which these small tables hold briefly, and the updates rewrite only the
-- rows of definitions with the owner section off. Give up rather than queue
-- behind a long transaction on any of them.
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

UPDATE factory_team_settings
SET standing_rules = array_append(standing_rules, 'quiet_gates')
WHERE standing_rules IS NOT NULL
    AND NOT standing_rules @> ARRAY['owner_gates']
    AND NOT standing_rules @> ARRAY['quiet_gates'];

UPDATE factory_project_rules
SET standing_rules = array_append(standing_rules, 'quiet_gates')
WHERE NOT standing_rules @> ARRAY['owner_gates']
    AND NOT standing_rules @> ARRAY['quiet_gates'];

UPDATE factory_run
SET standing_rules = array_append(standing_rules, 'quiet_gates')
WHERE standing_rules IS NOT NULL
    AND NOT standing_rules @> ARRAY['owner_gates']
    AND NOT standing_rules @> ARRAY['quiet_gates'];
