-- The Queue limit of the software factory's Orchestrator Definition
-- (crates/factory_records): the most Gates and questions that may wait on
-- a person before no new Run of theirs starts. A team sets it with its
-- Standing Rules, and again for one Project; null sets no limit. It goes
-- with the definition, so a team's row has none without Standing Rules.
--
-- Adding a nullable column rewrites no rows; adding each check scans its
-- table under its ACCESS EXCLUSIVE lock, which these small tables hold
-- briefly. Give up rather than queue behind a long transaction on either.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_team_settings
    ADD COLUMN queue_limit smallint,
    ADD CONSTRAINT factory_team_settings_queue_limit CHECK (
        queue_limit IS NULL
        OR (standing_rules IS NOT NULL AND queue_limit BETWEEN 0 AND 1000)
    );

ALTER TABLE factory_project_rules
    ADD COLUMN queue_limit smallint,
    ADD CONSTRAINT factory_project_rules_queue_limit CHECK (
        queue_limit BETWEEN 0 AND 1000
    );
