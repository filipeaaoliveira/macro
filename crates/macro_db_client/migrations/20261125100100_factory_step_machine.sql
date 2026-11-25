-- Where the software factory placed an agent Step (crates/factory_records,
-- `take_machine_slot`): the Machine (harness) whose slot the Step holds
-- while it runs, chosen before its session opens. A Step that has not
-- ended holds its slot; the slots a Machine has in use are its Steps with
-- this set that have not ended. No foreign key: harness rows are soft
-- deleted, and a hard delete must not wait on factory history.
--
-- While no Machine its Agents may run on has a free slot, the Step waits:
-- `slot_wait_since` is when it started to, and `slot_wait_machines` the
-- names of the full Machines it waits on. Both are cleared once it takes a
-- slot. Null for Steps no agent does, for Agents on Machines with no slot
-- limit, and for Steps from before this.
--
-- Adding nullable columns rewrites no row; the check scans factory_step
-- under its ACCESS EXCLUSIVE lock, which a small table holds briefly. Give
-- up rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step
    ADD COLUMN machine_id uuid,
    ADD COLUMN slot_wait_since timestamptz,
    ADD COLUMN slot_wait_machines text[] CHECK (cardinality(slot_wait_machines) <= 64);
