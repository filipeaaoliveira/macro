-- The factory's merge queue (crates/factory_records): the Runs that reached
-- their approved merge into a base branch, in the order they first reached
-- it, so Runs merge to one base one after the other; each update of a
-- Run's branch with its base, kept before the agent is asked to resolve a
-- conflict, so a retried Step goes on the way it started; and the head of
-- each Run's branch its review last passed on, which an update is told
-- apart from.
--
-- The tables are new and empty, and reference only factory_run, a table
-- few transactions touch: the SHARE ROW EXCLUSIVE lock their foreign keys
-- take on it is brief. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

-- A Run in the queue of the base branch it merges to. A base is its
-- repository's URL lower-cased, trimmed and without a `.git` ending, as
-- GitHub names one repository whatever the case, and its branch. place is
-- a serial taken as the Run first reached its merge, and keeps its place,
-- so two that reached it in the same instant keep the order they entered
-- in; entered_at is when that was. left_at is when it last left the merge
-- (to update its branch, or asked again), and is cleared when it comes
-- back. A Run that merged or ended leaves the queue. Times are the
-- database's.
CREATE TABLE factory_merge_queue (
    run_id uuid PRIMARY KEY REFERENCES factory_run (id) ON DELETE CASCADE,
    repo_url text NOT NULL CHECK (repo_url ~ '^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$'),
    base_branch text NOT NULL CHECK (octet_length(base_branch) BETWEEN 1 AND 255),
    place bigint GENERATED ALWAYS AS IDENTITY,
    entered_at timestamptz NOT NULL DEFAULT now(),
    left_at timestamptz
);

-- A base's queue is read in order.
CREATE INDEX factory_merge_queue_base_idx
    ON factory_merge_queue (repo_url, base_branch, place);

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

-- The head of the Run's branch its review last passed on, or the merge of
-- its base into that head GitHub made on the factory's asking: an update
-- of the branch ends without a review only when GitHub's merge has exactly
-- that head and the base's for parents. A Run without one is reviewed
-- again after any update. It goes with its Run.
CREATE TABLE factory_reviewed_head (
    run_id uuid PRIMARY KEY REFERENCES factory_run (id) ON DELETE CASCADE,
    head_sha text NOT NULL CHECK (head_sha ~ '^[0-9a-f]{40}$'),
    reviewed_at timestamptz NOT NULL DEFAULT now()
);
