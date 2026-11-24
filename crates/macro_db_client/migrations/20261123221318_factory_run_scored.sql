-- When the software factory finished scoring a Run that ended
-- (crates/factory_records, `mark_run_scored`): every live Scorer whose
-- sample took it judged it, or scoring gave up on the ones that did not.
-- `GET /runs/{id}/scores` then says no Scorer will still judge the Run.
-- Null while the Run goes, while it is scored, and for a Run that ended
-- before Runs kept it.
--
-- Adding a nullable column rewrites no rows. Give up rather than queue
-- behind a long transaction on the Runs.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run ADD COLUMN scored_at timestamptz;
