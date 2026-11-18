-- The Scorers, Experiments and Benchmarks of the Factory Definition a Run
-- of the software factory (crates/factory_records) started under, as the
-- Definition's `scorers.json` held them at the commit the Run read, so the
-- Scorers that judge the Run once it ends are the ones it started with.
-- Null for a Run whose team named no Definition, which the deployment's
-- Scorers judge, and for every Run created before this.
--
-- Adding a nullable column rewrites no rows. The check is added NOT VALID,
-- which takes the ACCESS EXCLUSIVE lock only briefly, and validated
-- after under a lock that lets reads and writes go on. Give up rather than
-- queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    ADD COLUMN definition_evaluation jsonb;

ALTER TABLE factory_run
    ADD CONSTRAINT factory_run_definition_evaluation CHECK (
        definition_evaluation IS NULL OR definition_commit IS NOT NULL
    ) NOT VALID;

ALTER TABLE factory_run VALIDATE CONSTRAINT factory_run_definition_evaluation;
