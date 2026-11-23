-- A person's label of a Scorer's judgment of one of their Runs
-- (crates/factory_records): whether the Run should have passed on the
-- Scorer's dimension. Insights compare the labels with the judge's to say
-- whether the Scorer can be trusted. A Run has at most one label per
-- Scorer, and a new label replaces it. Only the Run's owner labels it, so
-- the label's person is the Run's. It goes with the judgment it labels,
-- and so with its Run.
--
-- A label calibrates only the judge that made the judgment: the model and
-- the Scorer's instructions as it judged. A judgment keeps the SHA-256 of
-- its Scorer's instructions and reads (instructions_sha256, lowercase hex);
-- judgments made before have none, and calibrate no judge.
--
-- The new column and the new table's reference take an ACCESS EXCLUSIVE
-- lock on factory_run_score until this transaction ends. No existing row is
-- rewritten: the column has no default, and its check is validated apart
-- (factory_run_score_instructions_validate). Give up rather than queue
-- behind a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run_score
    ADD COLUMN instructions_sha256 text,
    ADD CONSTRAINT factory_run_score_instructions_sha256
        CHECK (instructions_sha256 ~ '^[0-9a-f]{64}$') NOT VALID;

CREATE TABLE factory_scorer_label (
    run_id uuid NOT NULL,
    scorer text NOT NULL,
    passed boolean NOT NULL,
    labelled_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, scorer),
    FOREIGN KEY (run_id, scorer)
        REFERENCES factory_run_score (run_id, scorer) ON DELETE CASCADE
);
