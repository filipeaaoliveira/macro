-- The Queue limit of the software factory's Orchestrator Definition
-- (crates/factory_records) is at least 1: a new Run starts only while its
-- owner holds fewer places than the limit, so a limit of 0 would start
-- none. The service refuses 0 before it is kept; these checks hold the
-- tables to the same range. No deployment keeps a limit of 0.
--
-- Swapping each check scans its table under its ACCESS EXCLUSIVE lock,
-- which these small tables hold briefly; no row is rewritten. Give up
-- rather than queue behind a long transaction on either.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_queue_limit,
    ADD CONSTRAINT factory_team_settings_queue_limit CHECK (
        queue_limit IS NULL
        OR (standing_rules IS NOT NULL AND queue_limit BETWEEN 1 AND 1000)
    );

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_queue_limit,
    ADD CONSTRAINT factory_project_rules_queue_limit CHECK (
        queue_limit BETWEEN 1 AND 1000
    );
