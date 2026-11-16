-- The Macro task a factory Run works on, at most one (crates/factory_records).
-- Relinking replaces it; deleting the task document unlinks it rather than
-- deleting the Run. factory_run is the factory's own and small, so the new
-- nullable column and its index lock it only briefly.
ALTER TABLE factory_run
    ADD COLUMN task_id text REFERENCES "Document" (id) ON DELETE SET NULL;

CREATE INDEX factory_run_task_id_idx ON factory_run (task_id) WHERE task_id IS NOT NULL;
