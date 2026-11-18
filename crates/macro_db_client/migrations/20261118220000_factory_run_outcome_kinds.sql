-- Two more ways a factory Run ends (crates/factory_records), so the
-- factory's quality reads apart from its plumbing: infrastructure, a Run
-- that failed for a machine, quota or network reason rather than for its
-- work; and flawed, a Run that ended success whose result a Scorer or a
-- person found wrong afterwards. A flawed Run keeps when and why it was
-- marked, and who marked it: the Run's owner, or a Scorer by its name.
-- Only the owner marks a Run in person, so a person's mark never names
-- anyone the Run's own row does not.
--
-- The new columns are nullable and rewrite no row; swapping the outcome
-- check and adding the flaw's scans factory_run under its ACCESS EXCLUSIVE
-- lock, which a small table holds briefly. Give up rather than queue
-- behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    DROP CONSTRAINT factory_run_outcome_check,
    ADD CONSTRAINT factory_run_outcome_check CHECK (
        outcome IN (
            'success', 'manual', 'blocked', 'failed', 'cancelled', 'infrastructure', 'flawed'
        )
    ),
    ADD COLUMN flawed_at timestamptz,
    ADD COLUMN flawed_by_user_id text,
    ADD COLUMN flawed_by_scorer text CHECK (flawed_by_scorer ~ '^[a-z0-9_-]{1,64}$'),
    ADD COLUMN flaw_reason text CHECK (octet_length(flaw_reason) BETWEEN 1 AND 4096),
    ADD CONSTRAINT factory_run_flaw CHECK (
        (flawed_at IS NOT NULL) = (outcome IS NOT DISTINCT FROM 'flawed')
        AND (flaw_reason IS NULL) = (flawed_at IS NULL)
        AND num_nonnulls(flawed_by_user_id, flawed_by_scorer)
            = CASE WHEN flawed_at IS NULL THEN 0 ELSE 1 END
        AND (flawed_by_user_id IS NULL OR flawed_by_user_id = user_id)
    );
