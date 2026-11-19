-- The factory's merge queue (crates/factory_records): the Runs that reached
-- their approved merge into a base branch, in the order they first reached
-- it, so Runs merge to one base one after the other; and each update of a
-- Run's branch with its base, kept before the agent is asked to resolve a
-- conflict, so a retried Step goes on the way it started.
--
-- Both tables are new and empty, and reference only factory_run, a table
-- few transactions touch: the SHARE ROW EXCLUSIVE lock their foreign keys
-- take on it is brief. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

-- A Run in the queue of the base branch it merges to. entered_at is when it
-- first reached its merge, and keeps its place; left_at is when it last left
-- the merge (to update its branch, or asked again), and is cleared when it
-- comes back. A Run that merged leaves the queue; one that ended is passed
-- over. Times are the database's.
CREATE TABLE factory_merge_queue (
    run_id uuid PRIMARY KEY REFERENCES factory_run (id) ON DELETE CASCADE,
    repo_url text NOT NULL CHECK (repo_url ~ '^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$'),
    base_branch text NOT NULL CHECK (octet_length(base_branch) BETWEEN 1 AND 255),
    entered_at timestamptz NOT NULL DEFAULT now(),
    left_at timestamptz
);

-- A base's queue is read in order.
CREATE INDEX factory_merge_queue_base_idx
    ON factory_merge_queue (repo_url, base_branch, entered_at, run_id);

-- An update Step's merge of its base into the Run's branch: the base's head
-- it merges, the branch's head it started from, whether GitHub could merge
-- them (no conflict) and, once it did, the branch's head GitHub's merge
-- made. It goes with its Run.
CREATE TABLE factory_branch_update (
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    seq integer NOT NULL CHECK (seq BETWEEN 1 AND 65535),
    base_sha text NOT NULL CHECK (base_sha ~ '^[0-9a-f]{40}$'),
    from_head text NOT NULL CHECK (from_head ~ '^[0-9a-f]{40}$'),
    conflict boolean NOT NULL,
    updated_head text CHECK (updated_head ~ '^[0-9a-f]{40}$'),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, seq),
    CONSTRAINT factory_branch_update_head CHECK (NOT (conflict AND updated_head IS NOT NULL))
);
