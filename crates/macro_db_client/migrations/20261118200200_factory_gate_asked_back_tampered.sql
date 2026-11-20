-- Whether the software factory asked back on words that approved a
-- Tampered Run (crates/factory_records): only the Approve button, which
-- lists the Run's changed Control Files, approves it. The Run view says so
-- beside how the Gate model read the words.
--
-- A NOT NULL column with a constant default rewrites no rows. Give up
-- rather than queue behind a long transaction on factory_gate.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_gate
    ADD COLUMN asked_back_refused_tampered boolean NOT NULL DEFAULT false;
