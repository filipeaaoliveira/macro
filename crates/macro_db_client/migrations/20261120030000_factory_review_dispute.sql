-- A fix's answer to a review round's Blockers, and the next round's ruling on
-- each it disputed (crates/factory_records). The fix hands back, per Blocker,
-- `fixed` or `disputed` with why (`fix_status`, `dispute`); the reviewer of
-- the next round who rules on a dispute upholds it, and the Blocker blocks
-- again, or drops it (`ruling`, by its Step, `ruled_by`). A finding a
-- reviewer raises again after it dropped it in the same visit of its entry
-- is kept as `dismissed` (`repeats_dropped`): it blocks nothing and becomes
-- no follow-up task.
--
-- Adding nullable columns, and one with a constant default, rewrites no
-- rows. The checks are added NOT VALID, and validated in
-- 20261120030100_factory_review_dispute_validate.sql, so the ACCESS
-- EXCLUSIVE lock is held for a moment only. Give up rather than queue behind
-- a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_review_finding
    ADD COLUMN fix_status text,
    ADD COLUMN dispute text,
    ADD COLUMN ruling text,
    ADD COLUMN ruled_by integer,
    ADD COLUMN repeats_dropped boolean NOT NULL DEFAULT false,
    DROP CONSTRAINT factory_review_finding_kind_check,
    ADD CONSTRAINT factory_review_finding_kind_check CHECK (
        kind IN ('blocker', 'follow_up', 'dismissed')
    ) NOT VALID,
    ADD CONSTRAINT factory_review_finding_dispute CHECK (
        (fix_status IS NULL OR fix_status IN ('fixed', 'disputed'))
        AND (dispute IS NOT NULL) = (fix_status IS NOT DISTINCT FROM 'disputed')
        AND (dispute IS NULL OR octet_length(dispute) BETWEEN 1 AND 2048)
        AND (ruling IS NULL OR ruling IN ('upheld', 'dropped'))
        AND (ruling IS NULL) = (ruled_by IS NULL)
        AND (ruling IS NULL OR fix_status = 'disputed')
    ) NOT VALID;
