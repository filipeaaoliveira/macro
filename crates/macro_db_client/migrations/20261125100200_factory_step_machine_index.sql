-- no-transaction

-- A Step's placement counts the Steps that hold a slot on a Machine and
-- have not ended (crates/factory_records, `take_machine_slot`). Built
-- concurrently, so Steps are recorded while it builds.
--
-- An interrupted build leaves an INVALID index, which IF NOT EXISTS would
-- then skip while the migration is recorded as applied. Before running this
-- migration again after one, drop it:
--   DROP INDEX CONCURRENTLY IF EXISTS factory_step_open_machine_idx;
CREATE INDEX CONCURRENTLY IF NOT EXISTS factory_step_open_machine_idx
    ON factory_step (machine_id)
    WHERE machine_id IS NOT NULL AND ended_at IS NULL;
