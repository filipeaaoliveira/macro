-- How the software factory's Gate model read a Gate when what it said
-- decided nothing (crates/factory_records): its pick, how sure it was and
-- why, when the stand-in left the Gate to the Run's owner, and when the
-- factory asked back on the owner's words. The owner reads them on the Run
-- to tell why.
--
-- Adding nullable columns rewrites no rows; the new check scans this small
-- table under its ACCESS EXCLUSIVE lock, briefly, and every row in place
-- passes it, with the new columns null. Give up rather than queue behind a
-- long transaction on factory_gate.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_gate
    -- When the stand-in, asked, left the Gate to the owner: the answer the
    -- model picked (null when it could not tell or did not answer), how
    -- sure it was in percent (null when it did not answer) and its reason
    -- (empty when it did not answer). All null when it was not asked, or
    -- answered.
    ADD COLUMN stand_in_declined_pick text,
    ADD COLUMN stand_in_declined_confidence smallint,
    ADD COLUMN stand_in_declined_reason text,
    -- The same for the words the factory last asked back on. Null for a
    -- reply asked back before the factory kept it.
    ADD COLUMN asked_back_pick text,
    ADD COLUMN asked_back_confidence smallint,
    ADD COLUMN asked_back_reason text;

ALTER TABLE factory_gate
    ADD CONSTRAINT factory_gate_model_readings CHECK (
        (stand_in_declined_reason IS NOT NULL OR (
            stand_in_declined_pick IS NULL AND stand_in_declined_confidence IS NULL
        ))
        AND (stand_in_declined_pick IS NULL OR stand_in_declined_pick = ANY (answers))
        AND stand_in_declined_confidence BETWEEN 0 AND 100
        AND octet_length(stand_in_declined_reason) <= 4096
        AND (asked_back_reason IS NOT NULL OR (
            asked_back_pick IS NULL AND asked_back_confidence IS NULL
        ))
        AND (asked_back_reason IS NULL OR asked_back_reply IS NOT NULL)
        AND (asked_back_pick IS NULL OR asked_back_pick = ANY (answers))
        AND asked_back_confidence BETWEEN 0 AND 100
        AND octet_length(asked_back_reason) <= 4096
    );
