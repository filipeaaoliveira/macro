-- The Flow each Run of the software factory (crates/factory_records) walks,
-- as it started: a built-in Flow or its team's Factory Definition's. What
-- a Gate's kind is, whether an entry is at the merge, and whether the Run
-- holds a place in its owner's Queue before its first Gate are read from
-- it, never from the Flows the factory or the Definition holds now.
--
-- `flow_asks`: whether the Flow has a Gate of its own, so a Run of it holds
-- a place in the Queue before its first Gate. Both null for Runs created
-- before this, which are looked up by their Flow's name, as before.
--
-- Adding nullable columns rewrites no rows; the lock on factory_run is
-- brief. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    ADD COLUMN flow_definition jsonb,
    ADD COLUMN flow_asks boolean;

ALTER TABLE factory_run
    ADD CONSTRAINT factory_run_flow_kept CHECK (
        (flow_definition IS NULL) = (flow_asks IS NULL)
    );
