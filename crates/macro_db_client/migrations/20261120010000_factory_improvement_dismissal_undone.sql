-- Undoing the dismissal of a rejected self-improvement Run's failures
-- (crates/factory_records): when a person rejected the proposed fix rather
-- than the failures, the Definition's namer or the Run's owner sends the
-- failures it dismissed back to pending. The undo is kept once on the
-- improvement, with who did it and when; repeating it changes nothing.
--
-- Adding nullable columns rewrites no row. Their check is added NOT VALID
-- and validated after, so no row is scanned under the ACCESS EXCLUSIVE lock.
-- Give up rather than queue behind a long transaction on the table.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_improvement
    ADD COLUMN dismissal_undone_at timestamptz,
    ADD COLUMN dismissal_undone_by_user_id text;

ALTER TABLE factory_improvement
    ADD CONSTRAINT factory_improvement_dismissal_undone CHECK (
        (dismissal_undone_at IS NULL) = (dismissal_undone_by_user_id IS NULL)
        AND (dismissal_undone_at IS NULL OR state = 'closed')
    ) NOT VALID;

ALTER TABLE factory_improvement
    VALIDATE CONSTRAINT factory_improvement_dismissal_undone;
