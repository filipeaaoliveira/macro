-- Gate replies and the stand-in in the software factory
-- (crates/factory_records): a person may answer a Gate in their own words,
-- which a small model reads as one of its answers or the factory asks back,
-- and a team may let a stand-in answer routine Gates for the Run's owner,
-- who can override it for a short while.
--
-- The new columns and checks change factory_gate, and the widened checks
-- of the stored Gate kinds change factory_run, factory_team_settings and
-- factory_project_rules, so this takes locks on them until the transaction
-- ends. Adding nullable columns, and a NOT NULL one with a constant default,
-- rewrites no rows; replacing the checks scans these small tables under
-- their ACCESS EXCLUSIVE lock, briefly. Give up rather than queue behind a
-- long transaction on any of them.
SET LOCAL lock_timeout = '5s';

-- A person's answer read from their words: how sure the factory was, in
-- percent. Null for an answer given with a button, and for every answer
-- not a person's.
ALTER TABLE factory_gate
    ADD COLUMN reply_confidence smallint,
    -- The words of the person's latest reply the factory could not read as
    -- one of the Gate's answers, and when it asked back for them; null when
    -- it never asked back.
    ADD COLUMN asked_back_reply text,
    ADD COLUMN asked_back_at timestamptz,
    -- The stand-in's answer for the Run's owner, its reason and how sure it
    -- was, in percent, and until when the owner may override it. All null
    -- when the stand-in did not answer.
    ADD COLUMN stand_in_answer text,
    ADD COLUMN stand_in_reason text,
    ADD COLUMN stand_in_confidence smallint,
    ADD COLUMN stand_in_until timestamptz,
    -- Whether the Gate's answer is the stand-in's, kept once the owner let
    -- the window pass.
    ADD COLUMN by_stand_in boolean NOT NULL DEFAULT false;

ALTER TABLE factory_gate
    ADD CONSTRAINT factory_gate_reply CHECK (
        (reply_confidence IS NULL OR (
            reply_confidence BETWEEN 0 AND 100
            AND answer IS NOT NULL
            AND rule IS NULL
            AND NOT by_stand_in
        ))
        AND (asked_back_reply IS NULL) = (asked_back_at IS NULL)
        AND octet_length(asked_back_reply) <= 4096
    ),
    ADD CONSTRAINT factory_gate_stand_in CHECK (
        (
            (stand_in_answer IS NULL)
            AND (stand_in_reason IS NULL)
            AND (stand_in_confidence IS NULL)
            AND (stand_in_until IS NULL)
            AND NOT by_stand_in
        )
        OR (
            stand_in_answer = ANY (answers)
            AND octet_length(stand_in_reason) <= 4096
            AND stand_in_confidence BETWEEN 0 AND 100
            AND stand_in_until IS NOT NULL
            AND (NOT by_stand_in OR (
                answer = stand_in_answer
                AND user_id IS NULL
                AND rule IS NULL
                AND note IS NULL
                AND approved_control_files IS NULL
            ))
        )
    );

-- The Orchestrator Definition also says which kinds of Gate the stand-in
-- answers (`stand_in_plan`, `stand_in_blockers`), beside the kinds the
-- Standing Rules decide. The stand-in never answers a merge Gate.
ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_standing_rules,
    ADD CONSTRAINT factory_run_standing_rules CHECK (
        (standing_rules IS NULL) = (standing_rules_source IS NULL)
        AND standing_rules_source IN ('default', 'team', 'project')
        AND (standing_rules_project_id IS NOT NULL) = (standing_rules_source = 'project')
        AND standing_rules <@ ARRAY['plan', 'blockers', 'merge', 'stand_in_plan', 'stand_in_blockers']
    );

ALTER TABLE factory_team_settings
    DROP CONSTRAINT factory_team_settings_standing_rules_check,
    ADD CONSTRAINT factory_team_settings_standing_rules_check CHECK (
        standing_rules <@ ARRAY['plan', 'blockers', 'merge', 'stand_in_plan', 'stand_in_blockers']
    );

ALTER TABLE factory_project_rules
    DROP CONSTRAINT factory_project_rules_standing_rules_check,
    ADD CONSTRAINT factory_project_rules_standing_rules_check CHECK (
        standing_rules <@ ARRAY['plan', 'blockers', 'merge', 'stand_in_plan', 'stand_in_blockers']
    );

-- The owner's past answers to a Gate of the same entry and Flow, which the
-- stand-in reads, are found through the owner's Runs
-- (`factory_run_user_created_idx`) and their Gates by primary key.
