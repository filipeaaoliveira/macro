-- The scheduler's configuration revision of an Automation's routine as the
-- factory last wrote it, on making the Automation or switching it on
-- (crates/factory_records). A run the scheduler started itself at this
-- revision proves, in the signed `scheduler_run` claim of its code's
-- credential, that it runs the trigger and code the factory wrote, and
-- fires the Automation as its creator. Automations made before this have
-- none until they are switched off and on.
--
-- A nullable column rewrites no row. Give up rather than queue behind a long
-- transaction on the table.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_automation
    ADD COLUMN routine_revision bigint;
