-- The token budget of the software factory's Orchestrator Definition
-- (crates/factory_records): the most tokens a Run's agent Steps may use, as
-- Insights counts them, before the Run ends `blocked`. A team sets it with
-- its Standing Rules, and again for one Project; null sets none. Unlike the
-- Queue limit, a Run keeps it with its rules, so a change to the definition
-- does not change a Run under way. A Run its budget ended keeps why
-- (`token_budget_stop`: `spent`, with the tokens it had used, or `unread`,
-- the usage log could not be read) for the Run API.
--
-- Adding nullable columns rewrites no rows. The settings tables are small,
-- and their checks scan them under an ACCESS EXCLUSIVE lock briefly; the
-- Runs' check is added NOT VALID, and validated in
-- 20261119050100_factory_token_budget_validate.sql. Give up rather than
-- queue behind a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_team_settings
    ADD COLUMN token_budget bigint,
    ADD CONSTRAINT factory_team_settings_token_budget CHECK (
        token_budget IS NULL
        OR (standing_rules IS NOT NULL AND token_budget BETWEEN 10000 AND 10000000000)
    );

ALTER TABLE factory_project_rules
    ADD COLUMN token_budget bigint,
    ADD CONSTRAINT factory_project_rules_token_budget CHECK (
        token_budget BETWEEN 10000 AND 10000000000
    );

ALTER TABLE factory_run
    ADD COLUMN token_budget bigint,
    ADD COLUMN token_budget_stop text,
    ADD COLUMN token_budget_used bigint,
    ADD CONSTRAINT factory_run_token_budget CHECK (
        (token_budget IS NULL OR (standing_rules IS NOT NULL AND token_budget > 0))
        AND (token_budget_stop IS NULL OR token_budget IS NOT NULL)
        AND (token_budget_stop IS NULL OR token_budget_stop IN ('spent', 'unread'))
        AND (token_budget_used IS NOT NULL) = (token_budget_stop IS NOT DISTINCT FROM 'spent')
        AND (token_budget_used IS NULL OR token_budget_used >= 0)
    ) NOT VALID;
