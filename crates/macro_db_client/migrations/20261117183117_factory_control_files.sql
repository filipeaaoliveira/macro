-- Control Files in the software factory (crates/factory_records): the files
-- that decide whether a repository's checks pass, such as its tests, lint
-- configuration and CI workflows. A Run whose branch changes one its request
-- does not ask for is Tampered, and merges only once a person approves the
-- files on the merge Gate.
--
-- The new columns change factory_run and factory_gate, and the new
-- references reach "team", so this takes locks on them until the transaction
-- ends. Adding nullable columns, or ones with a constant default, rewrites
-- no rows; adding the checks scans factory_run and factory_gate under their
-- ACCESS EXCLUSIVE lock, which these small tables hold briefly. Give up
-- rather than queue behind a long transaction on any of them.
SET LOCAL lock_timeout = '5s';

-- Whether the Run's owner created it in their own signed-in session, from
-- an account that is not an agent's: only then does its request ask for
-- the Control Files it names. Runs created before this count as not.
--
-- The Control File patterns the Run is checked against, and the team they
-- came from, as they were when it was created, so a change to the team's
-- list later does not change a Run under way. Null patterns (Runs created
-- before this) are checked against the factory's default. The team id is a
-- record of where they came from, not a reference: it outlives the team.
--
-- The Control Files the Run's branch last changed, as the pull request Steps
-- last read them: those its request does not ask for (`tampered_files`, the
-- Run is Tampered when there is one) and those it does
-- (`named_control_files`). Both null when it changed none.
-- `control_files_unlisted` when GitHub could not list the changes, or they
-- were too many to keep, so which Control Files it changed is not known.
ALTER TABLE factory_run
    ADD COLUMN created_in_person boolean NOT NULL DEFAULT false,
    ADD COLUMN control_file_patterns text[],
    ADD COLUMN control_files_team_id uuid,
    ADD COLUMN tampered_files text[],
    ADD COLUMN named_control_files text[],
    ADD COLUMN control_files_unlisted boolean NOT NULL DEFAULT false;

ALTER TABLE factory_run
    ADD CONSTRAINT factory_run_control_files CHECK (
        (tampered_files IS NULL) = (named_control_files IS NULL)
        AND (NOT control_files_unlisted OR tampered_files IS NOT NULL)
        AND cardinality(tampered_files) <= 300
        AND cardinality(named_control_files) <= 300
        AND cardinality(control_file_patterns) <= 200
    );

-- The Tampered files the person approved with their answer: the Run's
-- Tampered files when they answered, as they saw them. Null when the answer
-- approved none.
ALTER TABLE factory_gate
    ADD COLUMN approved_control_files text[];

ALTER TABLE factory_gate
    ADD CONSTRAINT factory_gate_approved_control_files CHECK (
        (approved_control_files IS NULL OR answer IS NOT NULL)
        AND cardinality(approved_control_files) <= 300
    );

-- A team's own list of Control File patterns, which its admins set and
-- which replaces the factory's default for its members' Runs. No row: the
-- default applies. It goes with the team.
CREATE TABLE factory_team_settings (
    team_id uuid PRIMARY KEY REFERENCES team (id) ON DELETE CASCADE,
    control_files text[] NOT NULL CHECK (cardinality(control_files) <= 200),
    updated_at timestamptz NOT NULL DEFAULT now()
);
