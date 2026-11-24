-- Where a factory agent Step's session ran (crates/factory_records,
-- `record_open`): the model the session ran on, and the name of the Machine
-- (the macrod harness its bot is bound to) it ran on. Both are read from
-- the session and its bot when the Step first sees the session open,
-- because the session is deleted once the Step is done with it, and a bot
-- may be bound to another Machine later. A Step that goes on in a new
-- session records that one's when it opens. Null for a Step no agent does,
-- for a session on no Machine (a runtime Macro runs), and for Steps from
-- before this.
--
-- Adding nullable columns rewrites no row; the checks scan factory_step
-- under its ACCESS EXCLUSIVE lock, which a small table holds briefly. Give
-- up rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step
    ADD COLUMN model text CHECK (octet_length(model) BETWEEN 1 AND 1024),
    ADD COLUMN machine text CHECK (octet_length(machine) BETWEEN 1 AND 1024);
