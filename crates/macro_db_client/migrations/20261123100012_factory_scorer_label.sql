-- A person's label of a Scorer's judgment of one of their Runs
-- (crates/factory_records): whether the Run should have passed on the
-- Scorer's dimension. Insights compare the labels with the judge's to say
-- whether the Scorer can be trusted. A Run has at most one label per
-- Scorer, and a new label replaces it. Only the Run's owner labels it, so
-- the label's person is the Run's. It goes with the judgment it labels,
-- and so with its Run.
--
-- The new table references factory_run_score, so creating it takes a SHARE
-- ROW EXCLUSIVE lock on factory_run_score until this transaction ends. No
-- existing row is rewritten. Give up rather than queue behind a long
-- transaction.
SET LOCAL lock_timeout = '5s';

CREATE TABLE factory_scorer_label (
    run_id uuid NOT NULL,
    scorer text NOT NULL,
    passed boolean NOT NULL,
    labelled_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, scorer),
    FOREIGN KEY (run_id, scorer)
        REFERENCES factory_run_score (run_id, scorer) ON DELETE CASCADE
);
