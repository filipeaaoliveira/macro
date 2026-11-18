-- The owner section of an Orchestrator Definition in the software factory
-- (crates/factory_records): with it on, a Gate no Standing Rule answers
-- goes to the Run's owner, who is notified, a review's Blockers found
-- unchanged after a rule's fix wait for them rather than another fix, and
-- their comments on the Run's task reach the next agent Step. A
-- definition is kept as the names it turns on, so the section is kept as
-- `owner_gates` beside the Gate kinds, in each column that keeps one.
--
-- Swapping each check scans its table under its ACCESS EXCLUSIVE lock,
-- and the index is built under a SHARE lock on factory_run, which these
-- small tables hold briefly; no row is rewritten. Give up
-- rather than queue behind a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_standing_rules,
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY['plan', 'blockers', 'merge', 'owner_gates']
    );

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_standing_rules_check,
    ADD CONSTRAINT factory_team_settings_standing_rules_check
        CHECK (standing_rules <@ ARRAY['plan', 'blockers', 'merge', 'owner_gates']);

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_standing_rules_check,
    ADD CONSTRAINT factory_project_rules_standing_rules_check
        CHECK (standing_rules <@ ARRAY['plan', 'blockers', 'merge', 'owner_gates']);


-- The Runs waiting on their owner are listed from the owner's Runs that
-- have not ended, newest first: an ended Run never waits, so the index
-- keeps none.
CREATE INDEX factory_run_user_open_idx
    ON factory_run (user_id, created_at DESC, id DESC)
    WHERE ended_at IS NULL;
