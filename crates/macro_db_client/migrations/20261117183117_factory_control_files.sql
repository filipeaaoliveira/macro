-- Control Files in the software factory (crates/factory_records): the files
-- that decide whether a repository's checks pass, such as its tests, lint
-- configuration and CI workflows. A Run whose branch changes one its request
-- does not name is Tampered, and merges only once a person approves the
-- files on the merge Gate.
--
-- The new columns change factory_run and factory_gate, and the new table
-- references "User", so this takes locks on them until the transaction
-- ends. Adding nullable columns rewrites no rows. Give up rather than queue
-- behind a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

-- The Control Files the Run's branch last changed that its request does not
-- name, as the pull request Steps last read them; null when it changed
-- none. Empty when GitHub could not list the branch's changes, so which
-- Control Files it changed is not known.
ALTER TABLE factory_run
    ADD COLUMN tampered_files text[] CHECK (cardinality(tampered_files) <= 300);

-- The Control Files the person approved with their answer: the Run's
-- Tampered files when they answered. Null when the answer approved none.
ALTER TABLE factory_gate
    ADD COLUMN approved_control_files text[] CHECK (cardinality(approved_control_files) <= 300),
    ADD CONSTRAINT factory_gate_approved_control_files CHECK (
        approved_control_files IS NULL OR answer IS NOT NULL
    );

-- A person's own list of Control File patterns, which replaces the
-- factory's default for their Runs. No row: the default applies. It goes
-- with their account.
CREATE TABLE factory_settings (
    user_id text PRIMARY KEY REFERENCES "User" (id) ON DELETE CASCADE,
    control_files text[] NOT NULL CHECK (cardinality(control_files) <= 200),
    updated_at timestamptz NOT NULL DEFAULT now()
);
