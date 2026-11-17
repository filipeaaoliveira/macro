-- An agent Step keeps the bot whose harness runs its session, so stopping
-- or deleting the session never depends on the factory's agent definition
-- still naming its Agent. Steps that claimed a session before this have
-- none.
--
-- Adding a nullable column rewrites no rows; the check scans factory_step
-- under its ACCESS EXCLUSIVE lock, which a small table holds briefly. Give
-- up rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step
    ADD COLUMN bot_id uuid,
    ADD CONSTRAINT factory_step_bot CHECK (bot_id IS NULL OR session_id IS NOT NULL);
