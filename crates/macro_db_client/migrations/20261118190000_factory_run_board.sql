-- The factory's board lists Runs across tasks (crates/factory_records):
-- by the Project their task was in when they started, and with the pull
-- request they opened.
--
-- project_id is the initiative the Run's task was in when the Run was
-- kept, set once: a record, not a reference, as the Run keeps the team its
-- Control Files came from. Null for a Run with no task, a task in no
-- Project, and every Run kept before this column.
--
-- pull_request_url is the page of the pull request the Run's branch
-- proposes its change in, kept by the Step that last found it. Null until
-- one is found, and for a Run that works on no code.
--
-- Adding nullable columns rewrites no rows. The check is added NOT VALID,
-- which scans nothing under the ACCESS EXCLUSIVE lock, and validated after
-- under a lock that lets reads and writes go on. Give up rather than queue
-- behind a long transaction on the table.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    ADD COLUMN project_id uuid,
    ADD COLUMN pull_request_url text,
    ADD CONSTRAINT factory_run_pull_request_url CHECK (
        pull_request_url LIKE 'https://%' AND octet_length(pull_request_url) <= 2048
    ) NOT VALID;

ALTER TABLE factory_run VALIDATE CONSTRAINT factory_run_pull_request_url;
