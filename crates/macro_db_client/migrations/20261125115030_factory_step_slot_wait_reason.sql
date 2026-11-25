-- Why an agent Step waits when it waits for no full Machine
-- (crates/factory_records, `wait_for_machine_slot`): none of the Machines
-- and API keys its Agents may run on for the Run's owner is there, as when
-- its Agent's Machine is someone else's (a machine's provider login is
-- spent only on its owner's Runs) or its Agent names a vendor the owner has
-- no machine or key for. Cleared, with `slot_wait_since`, once the Step
-- takes a slot or lets go of its wait. Null while it waits on full Machines,
-- and for Steps from before this.
--
-- Adding a nullable column rewrites no row; the check scans factory_step
-- under its ACCESS EXCLUSIVE lock, which a small table holds briefly. Give
-- up rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step
    ADD COLUMN slot_wait_reason text CHECK (char_length(slot_wait_reason) <= 2000);
